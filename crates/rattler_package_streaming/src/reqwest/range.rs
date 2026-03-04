//! Functionality to fetch package metadata from a remote `.conda` package using HTTP range requests.
//!
//! This module allows fetching just the `info/` section of a `.conda` package without downloading
//! the entire file. This is achieved by using HTTP Range requests to fetch only the necessary
//! bytes from the end of the zip archive.
//!
//! # Example
//!
//! ```rust,no_run
//! # #[tokio::main]
//! # async fn main() {
//! use rattler_conda_types::package::IndexJson;
//! use rattler_package_streaming::reqwest::range::fetch_package_file_from_url;
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//!
//! let client = ClientWithMiddleware::from(Client::new());
//! let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
//!
//! let index_json: IndexJson = fetch_package_file_from_url(client, url)
//!     .await
//!     .unwrap();
//!
//! println!("Package: {}", index_json.name.as_normalized());
//! # }
//! ```

use std::borrow::Cow;
use std::io::{Cursor, Read};

use async_zip::base::read::cd::{CentralDirectoryReader, Entry as CentralDirectoryEntryKind};
use async_zip::base::read::io::locator;
use async_zip::base::read::io::CombinedCentralDirectoryRecord;
use async_zip::spec::consts::{
    CDH_SIGNATURE, LFH_SIGNATURE, NON_ZIP64_MAX_SIZE, SIGNATURE_LENGTH, ZIP64_EOCDL_LENGTH,
    ZIP64_EOCDR_SIGNATURE,
};
use async_zip::spec::header::{
    EndOfCentralDirectoryHeader, LocalFileHeader, Zip64EndOfCentralDirectoryLocator,
    Zip64EndOfCentralDirectoryRecord,
};
use bytes::Bytes;
use futures::io::{
    AsyncReadExt as _, AsyncSeekExt as _, Cursor as AsyncCursor, SeekFrom as AsyncSeekFrom,
};
use http::StatusCode;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tar::Archive;
use tracing::debug;
use url::Url;

use crate::seek::read_package_file_content;
use crate::ExtractError;

/// Default number of bytes to fetch from the end of the file.
/// 64KB should be enough for most packages to include the EOCD, Central Directory,
/// and often the entire info archive.
const DEFAULT_TAIL_SIZE: u64 = 64 * 1024;

/// Size of a Local file header (signature + fixed fields, without variable fields)
const LOCAL_HEADER_SIZE: usize = SIGNATURE_LENGTH + 26;

/// Size of the ZIP64 EOCD record body (without signature).
const ZIP64_EOCDR_BODY_SIZE: usize = 52;

/// Size of ZIP64 EOCD record with signature.
const ZIP64_EOCDR_SIZE: u64 = SIGNATURE_LENGTH as u64 + ZIP64_EOCDR_BODY_SIZE as u64;

/// Information about a zip entry's location in the archive.
#[derive(Debug)]
struct ZipEntryLocation {
    /// Offset to the local file header from the start of the archive
    local_header_offset: u64,
    /// Compressed size of the file data
    compressed_size: u64,
}

/// Parsed Content-Range header information.
#[derive(Debug)]
struct ContentRange {
    /// Start byte position (inclusive)
    start: u64,
    /// Total file size
    total: u64,
}

impl ContentRange {
    /// Parse a Content-Range header value like "bytes 1000-2000/3000"
    fn parse(header_value: &str) -> Option<Self> {
        let header_value = header_value.strip_prefix("bytes ")?;
        let (range, total) = header_value.split_once('/')?;
        let (start, _end) = range.split_once('-')?;

        Some(ContentRange {
            start: start.parse().ok()?,
            total: total.parse().ok()?,
        })
    }
}

/// Result of a range request
enum RangeRequestResult {
    /// Successful range response with bytes and content range info
    Success(Bytes, ContentRange),
    /// Server doesn't support range requests (405 Method Not Allowed)
    NotSupported,
    /// Server returned full content (200 OK instead of 206)
    FullContent(Bytes),
}

/// Fetch bytes from a URL using HTTP Range header.
async fn fetch_range(
    client: &ClientWithMiddleware,
    url: &Url,
    range: &str,
) -> Result<RangeRequestResult, ExtractError> {
    debug!("fetching range {range} from {url}");

    let response = client
        .get(url.clone())
        .header(http::header::RANGE, range)
        .send()
        .await
        .map_err(ExtractError::ReqwestError)?;

    match response.status() {
        StatusCode::PARTIAL_CONTENT => {
            // Parse Content-Range header
            let content_range = response
                .headers()
                .get(http::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(ContentRange::parse)
                .ok_or_else(|| {
                    ExtractError::IoError(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "missing or invalid Content-Range header",
                    ))
                })?;

            let bytes = response
                .bytes()
                .await
                .map_err(|e| ExtractError::ReqwestError(e.into()))?;

            debug!(
                "received {} bytes (range {}-{}/{})",
                bytes.len(),
                content_range.start,
                content_range.start.saturating_add(bytes.len() as u64),
                content_range.total
            );

            Ok(RangeRequestResult::Success(bytes, content_range))
        }
        StatusCode::METHOD_NOT_ALLOWED => {
            debug!("server does not support range requests (405)");
            Ok(RangeRequestResult::NotSupported)
        }
        StatusCode::OK => {
            // Server doesn't support range requests, returned full content
            let bytes = response
                .bytes()
                .await
                .map_err(|e| ExtractError::ReqwestError(e.into()))?;
            debug!(
                "server returned full content ({} bytes) instead of range",
                bytes.len()
            );
            Ok(RangeRequestResult::FullContent(bytes))
        }
        status => {
            debug!("range request failed with status {status}");
            let error = response
                .error_for_status()
                .expect_err("non-success status should error");
            Err(ExtractError::ReqwestError(error.into()))
        }
    }
}

/// Builds a `Range` header value for a byte segment.
fn build_range_header(start: u64, len: u64) -> Option<String> {
    let end = start.checked_add(len)?.checked_sub(1)?;
    Some(format!("bytes={start}-{end}"))
}

/// Fetches a fixed byte segment, returning `None` if the server cannot satisfy range requests
/// or if the requested segment is invalid.
async fn fetch_range_bytes(
    client: &ClientWithMiddleware,
    url: &Url,
    start: u64,
    len: u64,
) -> Result<Option<Bytes>, ExtractError> {
    let Some(range) = build_range_header(start, len) else {
        return Ok(None);
    };

    match fetch_range(client, url, &range).await? {
        RangeRequestResult::Success(bytes, content_range) => {
            let Some(expected_end) = start.checked_add(len) else {
                return Ok(None);
            };
            let Some(actual_end) = content_range.start.checked_add(bytes.len() as u64) else {
                return Ok(None);
            };

            if content_range.start != start || actual_end != expected_end {
                debug!(
                    "server returned mismatched range for {range}: got start={}, len={}, expected start={start}, len={len}",
                    content_range.start,
                    bytes.len()
                );
                return Ok(None);
            }

            Ok(Some(bytes))
        }
        RangeRequestResult::NotSupported | RangeRequestResult::FullContent(_) => Ok(None),
    }
}

/// Parse central-directory metadata from tail bytes, including ZIP64 records.
/// Returns `None` when metadata cannot be parsed from range requests.
async fn find_central_directory_record(
    client: &ClientWithMiddleware,
    url: &Url,
    tail_bytes: &Bytes,
    tail_start_offset: u64,
) -> Result<Option<CombinedCentralDirectoryRecord>, ExtractError> {
    let mut cursor = AsyncCursor::new(&tail_bytes[..]);

    let eocdr_offset = match locator::eocdr(&mut cursor).await {
        Ok(offset) => offset,
        Err(_) => return Ok(None),
    };

    if cursor
        .seek(AsyncSeekFrom::Start(eocdr_offset))
        .await
        .is_err()
    {
        return Ok(None);
    }

    let eocdr = match EndOfCentralDirectoryHeader::from_reader(&mut cursor).await {
        Ok(record) => record,
        Err(_) => return Ok(None),
    };

    let maybe_zip64 = eocdr.disk_num == u16::MAX
        || eocdr.start_cent_dir_disk == u16::MAX
        || eocdr.num_of_entries_disk == u16::MAX
        || eocdr.num_of_entries == u16::MAX
        || eocdr.size_cent_dir == NON_ZIP64_MAX_SIZE
        || eocdr.cent_dir_offset == NON_ZIP64_MAX_SIZE;

    if !maybe_zip64 {
        return Ok(Some(CombinedCentralDirectoryRecord::from(&eocdr)));
    }

    let Some(zip64_locator_offset) =
        eocdr_offset.checked_sub(ZIP64_EOCDL_LENGTH + SIGNATURE_LENGTH as u64)
    else {
        return Ok(None);
    };

    if cursor
        .seek(AsyncSeekFrom::Start(zip64_locator_offset))
        .await
        .is_err()
    {
        return Ok(None);
    }

    let zip64_locator = match Zip64EndOfCentralDirectoryLocator::try_from_reader(&mut cursor).await
    {
        Ok(Some(locator)) => locator,
        _ => return Ok(None),
    };

    let zip64_record_bytes = if let Some(bytes) = slice_from_tail(
        tail_bytes,
        tail_start_offset,
        zip64_locator.relative_offset,
        ZIP64_EOCDR_SIZE,
    ) {
        bytes
    } else {
        let Some(bytes) =
            fetch_range_bytes(client, url, zip64_locator.relative_offset, ZIP64_EOCDR_SIZE).await?
        else {
            return Ok(None);
        };
        bytes
    };

    let Some(zip64_eocdr) = parse_zip64_eocdr(&zip64_record_bytes) else {
        return Ok(None);
    };

    Ok(Some(CombinedCentralDirectoryRecord::combine(
        eocdr,
        zip64_eocdr,
    )))
}

/// Parse Central Directory entries to find the info-*.tar.zst file.
async fn find_info_entry(
    cd_bytes: &[u8],
    cd_start_in_file: u64,
    num_entries: u64,
) -> Option<ZipEntryLocation> {
    if cd_bytes.len() < SIGNATURE_LENGTH {
        return None;
    }

    let mut cursor = AsyncCursor::new(cd_bytes);
    let mut signature = [0; SIGNATURE_LENGTH];
    cursor.read_exact(&mut signature).await.ok()?;

    if u32::from_le_bytes(signature) != CDH_SIGNATURE {
        return None;
    }

    let mut reader = CentralDirectoryReader::new(&mut cursor, cd_start_in_file);

    for _ in 0..num_entries {
        let entry = reader.next().await.ok()?;
        let CentralDirectoryEntryKind::CentralDirectoryEntry(entry) = entry else {
            break;
        };

        let filename = entry.filename().as_bytes();
        if filename.starts_with(b"info-") && filename.ends_with(b".tar.zst") {
            return Some(ZipEntryLocation {
                local_header_offset: entry.file_offset(),
                compressed_size: entry.compressed_size(),
            });
        }
    }

    None
}

/// Parse a ZIP64 EOCD record from bytes that include its signature.
fn parse_zip64_eocdr(bytes: &[u8]) -> Option<Zip64EndOfCentralDirectoryRecord> {
    if bytes.len() < ZIP64_EOCDR_SIZE as usize {
        return None;
    }

    let signature = u32::from_le_bytes(bytes[0..SIGNATURE_LENGTH].try_into().ok()?);
    if signature != ZIP64_EOCDR_SIGNATURE {
        return None;
    }

    let payload: [u8; ZIP64_EOCDR_BODY_SIZE] = bytes
        [SIGNATURE_LENGTH..SIGNATURE_LENGTH + ZIP64_EOCDR_BODY_SIZE]
        .try_into()
        .ok()?;

    Some(Zip64EndOfCentralDirectoryRecord::from(payload))
}

/// Calculate the data offset from a local file header.
/// Returns the offset where the actual file data starts.
fn get_data_offset_from_local_header(header_bytes: &[u8]) -> Option<u64> {
    if header_bytes.len() < LOCAL_HEADER_SIZE {
        return None;
    }

    let sig = u32::from_le_bytes([
        header_bytes[0],
        header_bytes[1],
        header_bytes[2],
        header_bytes[3],
    ]);

    if sig != LFH_SIGNATURE {
        return None;
    }

    // Parse local file header using astral_async_zip (26 bytes after signature)
    let lfh_bytes: [u8; 26] = header_bytes[SIGNATURE_LENGTH..LOCAL_HEADER_SIZE]
        .try_into()
        .ok()?;
    let lfh = LocalFileHeader::from(lfh_bytes);

    Some(
        LOCAL_HEADER_SIZE as u64
            + u64::from(lfh.file_name_length)
            + u64::from(lfh.extra_field_length),
    )
}

/// Try to extract a slice from tail bytes if the requested range is fully contained within it.
/// Returns `None` if the range is outside or only partially within the tail bytes.
fn slice_from_tail(
    tail_bytes: &Bytes,
    tail_start_offset: u64,
    offset: u64,
    len: u64,
) -> Option<Bytes> {
    if offset < tail_start_offset {
        return None;
    }

    let start_in_tail = usize::try_from(offset - tail_start_offset).ok()?;
    let len = usize::try_from(len).ok()?;
    let end_in_tail = start_in_tail.checked_add(len)?;
    if end_in_tail <= tail_bytes.len() {
        Some(tail_bytes.slice(start_in_tail..end_in_tail))
    } else {
        None
    }
}

/// Extract a file from a tar archive bytes.
fn extract_file_from_tar<P: PackageFile>(tar_bytes: &[u8]) -> Result<P, ExtractError> {
    let cursor = Cursor::new(tar_bytes);
    let mut archive = Archive::new(cursor);

    let target_path = P::package_path();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;

        if path == target_path {
            let mut contents = String::new();
            entry.read_to_string(&mut contents)?;
            return P::from_str(&contents)
                .map_err(|e| ExtractError::ArchiveMemberParseError(target_path.to_path_buf(), e));
        }
    }

    Err(ExtractError::MissingComponent)
}

/// Fetch a specific [`PackageFile`] from a remote package using HTTP range requests.
///
/// This function fetches only the minimal bytes needed from the package, typically
/// just the `info/` section which is located at the end of the `.conda` archive.
///
/// If the server returns 405 (Method Not Allowed) or the package is not a `.conda` file,
/// the function falls back to downloading the entire package.
///
/// # Arguments
///
/// * `client` - The HTTP client to use for requests
/// * `url` - The URL of the package
///
/// # Returns
///
/// The parsed package file (e.g., `IndexJson`, `AboutJson`, etc.)
///
/// # Example
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// use rattler_conda_types::package::IndexJson;
/// use rattler_package_streaming::reqwest::range::fetch_package_file_from_url;
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// use url::Url;
///
/// let client = ClientWithMiddleware::from(Client::new());
/// let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
///
/// let index_json: IndexJson = fetch_package_file_from_url(client, url)
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn fetch_package_file_from_url<P: PackageFile>(
    client: ClientWithMiddleware,
    url: Url,
) -> Result<P, ExtractError> {
    debug!(
        "fetching {} from {} using range requests",
        P::package_path().display(),
        url
    );

    // Determine archive type from URL - only .conda supports efficient range requests
    let archive_type = CondaArchiveType::try_from(std::path::Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?;

    if archive_type != CondaArchiveType::Conda {
        // .tar.bz2 files don't support efficient range requests, fall back to full download
        debug!("archive type is .tar.bz2, falling back to full download");
        return fetch_package_file_full_download(&client, &url, archive_type).await;
    }

    // Step 1: Fetch the tail of the file
    let range = format!("bytes=-{DEFAULT_TAIL_SIZE}");
    let tail_result = fetch_range(&client, &url, &range).await?;

    let (tail_bytes, content_range) = match tail_result {
        RangeRequestResult::Success(bytes, range) => (bytes, range),
        RangeRequestResult::NotSupported => {
            debug!("server does not support range requests, falling back to full download");
            return fetch_package_file_full_download(&client, &url, CondaArchiveType::Conda).await;
        }
        RangeRequestResult::FullContent(bytes) => {
            // Server returned full content, extract from that
            debug!("server returned full content, extracting from response");
            let content = read_package_file_content(
                Cursor::new(&*bytes),
                CondaArchiveType::Conda,
                P::package_path(),
            )?;
            return P::from_str(&String::from_utf8_lossy(&content)).map_err(|e| {
                ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e)
            });
        }
    };

    // Calculate where the tail starts in the full file.
    // Validate that the response covers from start to the end of the file.
    let tail_start_offset = content_range.start;
    let Some(tail_end_offset) = tail_start_offset.checked_add(tail_bytes.len() as u64) else {
        return Err(ExtractError::ZipError(
            zip::result::ZipError::InvalidArchive(Cow::Borrowed(
                "Content-Range overflow while validating response body length",
            )),
        ));
    };
    if tail_end_offset != content_range.total {
        return Err(ExtractError::ZipError(
            zip::result::ZipError::InvalidArchive(Cow::Borrowed(
                "Content-Range does not match response body length",
            )),
        ));
    }

    // Step 2: Parse central-directory metadata from the tail (including ZIP64 records).
    let Some(cd_record) =
        find_central_directory_record(&client, &url, &tail_bytes, tail_start_offset).await?
    else {
        debug!(
            "could not parse central directory metadata from tail, falling back to full download"
        );
        return fetch_package_file_full_download(&client, &url, CondaArchiveType::Conda).await;
    };

    // Step 3: Check if Central Directory is in our tail bytes.
    let cd_start_in_file = cd_record.central_directory_offset();
    let cd_size = cd_record.directory_size;
    let num_entries = cd_record.num_entries();
    debug!(
        "central directory: offset={cd_start_in_file}, size={cd_size}, entries={num_entries}, total_file_size={}",
        content_range.total,
    );

    let cd_bytes = if let Some(bytes) =
        slice_from_tail(&tail_bytes, tail_start_offset, cd_start_in_file, cd_size)
    {
        debug!("central directory found in tail bytes");
        bytes
    } else {
        // CD is not (fully) in our tail, need to fetch it
        debug!("central directory not in tail, fetching separately");
        match fetch_range_bytes(&client, &url, cd_start_in_file, cd_size).await? {
            Some(bytes) => bytes,
            None => {
                return fetch_package_file_full_download(&client, &url, CondaArchiveType::Conda)
                    .await;
            }
        }
    };

    // Step 4: Find the info-*.tar.zst entry in the Central Directory.
    let entry = find_info_entry(&cd_bytes, cd_start_in_file, num_entries)
        .await
        .ok_or(ExtractError::MissingComponent)?;
    debug!(
        "found info archive entry: local_header_offset={}, compressed_size={}",
        entry.local_header_offset, entry.compressed_size
    );

    // Step 5: We need to read the local file header to get the actual data offset
    // The local header has variable-length fields that may differ from CD
    let local_header_offset = entry.local_header_offset;

    // Check if local header is in our tail bytes
    let local_header_bytes = if let Some(bytes) = slice_from_tail(
        &tail_bytes,
        tail_start_offset,
        local_header_offset,
        LOCAL_HEADER_SIZE as u64,
    ) {
        bytes
    } else {
        // Need to fetch local header
        debug!("local header not in tail, fetching separately");
        match fetch_range_bytes(&client, &url, local_header_offset, LOCAL_HEADER_SIZE as u64)
            .await?
        {
            Some(bytes) => bytes,
            None => {
                return fetch_package_file_full_download(&client, &url, CondaArchiveType::Conda)
                    .await;
            }
        }
    };

    let data_offset_from_header =
        get_data_offset_from_local_header(&local_header_bytes).ok_or(ExtractError::ZipError(
            zip::result::ZipError::InvalidArchive(Cow::Borrowed("invalid local file header")),
        ))?;

    let Some(data_start) = local_header_offset.checked_add(data_offset_from_header) else {
        return Err(ExtractError::ZipError(
            zip::result::ZipError::InvalidArchive(Cow::Borrowed(
                "info archive start offset overflow",
            )),
        ));
    };

    // Step 6: Fetch the info archive data (if not already in tail)
    let info_archive_bytes = if let Some(bytes) = slice_from_tail(
        &tail_bytes,
        tail_start_offset,
        data_start,
        entry.compressed_size,
    ) {
        debug!("info archive data found in tail bytes");
        bytes
    } else {
        // Need to fetch the info archive
        debug!("info archive data not in tail, fetching separately");
        match fetch_range_bytes(&client, &url, data_start, entry.compressed_size).await? {
            Some(bytes) => bytes,
            None => {
                return fetch_package_file_full_download(&client, &url, CondaArchiveType::Conda)
                    .await;
            }
        }
    };

    // Step 7: Decompress zstd
    debug!(
        "decompressing {} bytes of zstd-compressed info archive",
        info_archive_bytes.len()
    );
    let tar_bytes =
        zstd::decode_all(Cursor::new(&info_archive_bytes[..])).map_err(ExtractError::IoError)?;
    debug!(
        "decompressed to {} bytes, extracting {}",
        tar_bytes.len(),
        P::package_path().display()
    );

    // Step 8: Extract the specific file from tar
    extract_file_from_tar::<P>(&tar_bytes)
}

/// Download full package and extract [`PackageFile`] when range requests fail.
async fn fetch_package_file_full_download<P: PackageFile>(
    client: &ClientWithMiddleware,
    url: &Url,
    archive_type: CondaArchiveType,
) -> Result<P, ExtractError> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(ExtractError::ReqwestError)?
        .error_for_status()
        .map_err(|e| ExtractError::ReqwestError(e.into()))?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ExtractError::ReqwestError(e.into()))?;

    let content = read_package_file_content(Cursor::new(&*bytes), archive_type, P::package_path())?;
    P::from_str(&String::from_utf8_lossy(&content))
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_content_range() {
        let range = ContentRange::parse("bytes 1000-2000/3000").unwrap();
        assert_eq!(range.start, 1000);
        assert_eq!(range.total, 3000);
    }

    #[test]
    fn test_parse_content_range_invalid() {
        assert!(ContentRange::parse("invalid").is_none());
        assert!(ContentRange::parse("bytes 1000-2000").is_none());
    }

    #[test]
    fn test_build_range_header() {
        assert_eq!(build_range_header(10, 10), Some("bytes=10-19".to_string()));
    }

    #[test]
    fn test_build_range_header_invalid() {
        assert_eq!(build_range_header(0, 0), None);
        assert_eq!(build_range_header(u64::MAX, 2), None);
    }

    #[test]
    fn test_parse_zip64_eocdr() {
        let zip64_eocdr: [u8; 56] = [
            0x50, 0x4B, 0x06, 0x06, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1E, 0x03,
            0x2D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2F, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let parsed = parse_zip64_eocdr(&zip64_eocdr).unwrap();
        assert_eq!(parsed.directory_size, 47);
        assert_eq!(parsed.offset_of_start_of_directory, 64);
    }

    #[test]
    fn test_parse_zip64_eocdr_invalid_signature() {
        let mut zip64_eocdr = [0u8; 56];
        zip64_eocdr[0] = 0xAA;
        assert!(parse_zip64_eocdr(&zip64_eocdr).is_none());
    }

    #[tokio::test]
    async fn test_fetch_index_json_from_conda_forge() {
        use rattler_conda_types::package::IndexJson;

        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        let url = Url::parse(
            "https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda",
        )
        .unwrap();

        let index_json: IndexJson = fetch_package_file_from_url(client, url).await.unwrap();

        assert_eq!(index_json.name.as_normalized(), "tzdata");
        assert_eq!(index_json.version.to_string(), "2024b");
    }

    #[tokio::test]
    async fn test_fetch_about_json_from_conda_forge() {
        use rattler_conda_types::package::AboutJson;

        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        let url = Url::parse(
            "https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda",
        )
        .unwrap();

        let about_json: AboutJson = fetch_package_file_from_url(client, url).await.unwrap();

        // tzdata package should have license info
        assert!(about_json.license.is_some());
    }
}
