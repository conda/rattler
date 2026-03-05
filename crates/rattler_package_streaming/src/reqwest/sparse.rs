//! A layered API for fetching multiple files from remote packages efficiently.
//!
//! [`SparseRemoteArchive`] eagerly fetches and caches the decompressed `info/` section on
//! construction using HTTP range requests. All subsequent reads are from cache (zero I/O),
//! making it efficient to read multiple info files from the same package.
//!
//! Only `.conda` archives on servers that support range requests are supported.
//! For a higher-level API that falls back to full downloads, see
//! [`super::range::fetch_package_file_from_url`].
//!
//! # Example
//!
//! ```rust,no_run
//! # #[tokio::main]
//! # async fn main() {
//! use rattler_conda_types::package::{AboutJson, IndexJson};
//! use rattler_package_streaming::reqwest::sparse::SparseRemoteArchive;
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//!
//! let client = ClientWithMiddleware::from(Client::new());
//! let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
//!
//! let archive = SparseRemoteArchive::new(client, url).await.unwrap();
//! let index_json: IndexJson = archive.read().unwrap();
//! let about_json: AboutJson = archive.read().unwrap();
//! # }
//! ```

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use async_http_range_reader::{AsyncHttpRangeReader, CheckSupportMethod};
use async_zip::base::read::seek::ZipFileReader;
use futures::io::AsyncReadExt as _;
use http::HeaderMap;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tar::Archive;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::debug;
use url::Url;

use crate::ExtractError;

/// Default number of bytes to fetch from the end of the file.
/// 64KB should be enough for most packages to include the EOCD, Central Directory,
/// and often the entire info archive.
const DEFAULT_TAIL_SIZE: u64 = 64 * 1024;

/// A remote `.conda` archive with the `info/` section cached in memory.
///
/// Construct once via [`SparseRemoteArchive::new`], then read multiple files
/// without redundant HTTP requests or decompression.
///
/// Only `.conda` archives on servers that support HTTP range requests are
/// supported. Returns an error for `.tar.bz2` archives or when the server
/// does not support range requests.
///
/// # Example
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// use rattler_conda_types::package::IndexJson;
/// use rattler_package_streaming::reqwest::sparse::SparseRemoteArchive;
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// use url::Url;
///
/// let client = ClientWithMiddleware::from(Client::new());
/// let url = Url::parse("https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda").unwrap();
///
/// let archive = SparseRemoteArchive::new(client, url).await.unwrap();
/// let index_json: IndexJson = archive.read().unwrap();
/// println!("Package: {}", index_json.name.as_normalized());
/// # }
/// ```
pub struct SparseRemoteArchive {
    files: HashMap<PathBuf, Vec<u8>>,
}

impl SparseRemoteArchive {
    /// Open a remote `.conda` archive and cache its info section using HTTP
    /// range requests.
    ///
    /// Returns an error if the URL does not point to a `.conda` archive or
    /// the server does not support range requests.
    pub async fn new(client: ClientWithMiddleware, url: Url) -> Result<Self, ExtractError> {
        debug!("opening sparse remote archive for {}", url);

        let archive_type = CondaArchiveType::try_from(std::path::Path::new(url.path()))
            .ok_or(ExtractError::UnsupportedArchiveType)?;

        if archive_type != CondaArchiveType::Conda {
            return Err(ExtractError::UnsupportedArchiveType);
        }

        // Create range reader (fetches last 64KB on construction)
        let (reader, _headers) = AsyncHttpRangeReader::new(
            client,
            url,
            CheckSupportMethod::NegativeRangeRequest(DEFAULT_TAIL_SIZE),
            HeaderMap::default(),
        )
        .await?;

        // Wrap for async_zip (tokio → futures traits + buffering)
        let buf_reader = futures::io::BufReader::new(reader.compat());

        // Open ZIP (parses EOCD + central directory, data already cached from range request)
        let mut zip_reader = ZipFileReader::new(buf_reader).await?;

        // Find the info-*.tar.zst entry
        let (index, _entry) = zip_reader
            .file()
            .entries()
            .iter()
            .enumerate()
            .find(|(_, e)| {
                e.filename()
                    .as_str()
                    .map(|f| f.starts_with("info-") && f.ends_with(".tar.zst"))
                    .unwrap_or(false)
            })
            .ok_or(ExtractError::MissingComponent)?;

        // Read the entry (async_zip handles seek + local header parsing)
        let mut entry_reader = zip_reader.reader_without_entry(index).await?;
        let mut compressed_data = Vec::new();
        entry_reader.read_to_end(&mut compressed_data).await?;

        // Decompress zstd → extract all tar entries
        debug!(
            "decompressing {} bytes of zstd-compressed info archive",
            compressed_data.len()
        );
        let tar_bytes =
            zstd::decode_all(Cursor::new(&compressed_data)).map_err(ExtractError::IoError)?;
        debug!(
            "decompressed to {} bytes, extracting all info entries",
            tar_bytes.len()
        );

        let files = extract_all_from_tar(&tar_bytes)?;
        Ok(Self { files })
    }

    /// List all cached file paths (e.g. `info/index.json`, `info/about.json`, etc.).
    pub fn entries(&self) -> impl Iterator<Item = &Path> {
        self.files.keys().map(PathBuf::as_path)
    }

    /// Read raw bytes of a cached file.
    pub fn read_raw(&self, path: &Path) -> Result<&[u8], ExtractError> {
        self.files
            .get(path)
            .map(Vec::as_slice)
            .ok_or(ExtractError::MissingComponent)
    }

    /// Read and parse a typed [`PackageFile`].
    pub fn read<P: PackageFile>(&self) -> Result<P, ExtractError> {
        let path = P::package_path();
        let bytes = self.read_raw(path)?;
        P::from_str(&String::from_utf8_lossy(bytes))
            .map_err(|e| ExtractError::ArchiveMemberParseError(path.to_path_buf(), e))
    }
}

/// Extract all entries from a tar archive into a map of path → bytes.
fn extract_all_from_tar(tar_bytes: &[u8]) -> Result<HashMap<PathBuf, Vec<u8>>, ExtractError> {
    let cursor = Cursor::new(tar_bytes);
    let mut archive = Archive::new(cursor);
    let mut files = HashMap::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        files.insert(path, buf);
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sparse_remote_archive() {
        use rattler_conda_types::package::{AboutJson, IndexJson};

        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        let url = Url::parse(
            "https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda",
        )
        .unwrap();

        let archive = SparseRemoteArchive::new(client, url).await.unwrap();

        // Verify entries are present
        let entries: Vec<_> = archive.entries().collect();
        assert!(!entries.is_empty());
        assert!(entries.iter().any(|p| *p == Path::new("info/index.json")));

        // Read typed files
        let index_json: IndexJson = archive.read().unwrap();
        assert_eq!(index_json.name.as_normalized(), "tzdata");

        let about_json: AboutJson = archive.read().unwrap();
        assert!(about_json.license.is_some());

        // Read raw bytes
        let raw = archive.read_raw(Path::new("info/index.json")).unwrap();
        assert!(!raw.is_empty());
    }
}
