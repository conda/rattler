//! Fetch individual files from remote `.conda` packages using HTTP range requests.
//!
//! Streams the zstd-compressed info tar archive directly from the server,
//! decompressing on the fly and stopping as soon as the target file is found.
//! This means only the bytes up to (and including) the target file are ever
//! downloaded or decompressed — even if the info archive is very large.
//!
//! Only `.conda` archives on servers that support range requests are supported.
//! For a higher-level API that falls back to full downloads, see
//! [`super::fetch::fetch_package_file_from_url`].
//!
//! # Example
//!
//! ```rust,no_run
//! # #[tokio::main]
//! # async fn main() {
//! use rattler_conda_types::package::IndexJson;
//! use rattler_package_streaming::reqwest::sparse::fetch_package_file_from_remote_sparse;
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//!
//! let client = ClientWithMiddleware::from(Client::new());
//! let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
//!
//! let index_json: IndexJson = fetch_package_file_from_remote_sparse(client, url).await.unwrap();
//! # }
//! ```

use std::path::Path;

use async_compression::tokio::bufread::{BzDecoder, ZstdDecoder};
use async_http_range_reader::{AsyncHttpRangeReader, CheckSupportMethod};
use async_zip::base::read::seek::ZipFileReader;
use futures::TryStreamExt;
use http::HeaderMap;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use rattler_redaction::{redact_known_secrets_from_url, DEFAULT_REDACTION_STR};
use reqwest_middleware::ClientWithMiddleware;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tokio_util::io::StreamReader;
use tracing::{debug, instrument};
use url::Url;

use crate::tokio::async_read::{conda_entry_prefix, get_file_from_tar_archive};
use crate::ExtractError;

/// Default number of bytes to fetch from the end of the file.
/// 64KB should be enough for most packages to include the EOCD, Central Directory,
/// and often the entire info archive.
const DEFAULT_TAIL_SIZE: u64 = 64 * 1024;

/// Fetch the raw bytes of a single file from a remote `.conda` package using
/// HTTP range requests.
///
/// Streams the zstd data directly from the server through an async decompressor
/// and tar reader, stopping as soon as the target file is found. Only the bytes
/// needed to reach the target file are downloaded and decompressed.
///
/// Returns `Ok(None)` if the file is not found in the archive.
/// Returns an error if the URL does not point to a `.conda` archive or the
/// server does not support range requests.
#[instrument(skip_all, fields(url = %redact_known_secrets_from_url(&url, DEFAULT_REDACTION_STR).as_ref().unwrap_or(&url), path = %target_path.display()))]
pub async fn fetch_file_from_remote_sparse(
    client: ClientWithMiddleware,
    url: Url,
    target_path: &Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
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

    // Find the tar.zst entry that contains the target path
    let prefix = conda_entry_prefix(target_path);
    let (index, _) = zip_reader
        .file()
        .entries()
        .iter()
        .enumerate()
        .find(|(_, e)| {
            e.filename()
                .as_str()
                .map(|f| f.starts_with(prefix) && f.ends_with(".tar.zst"))
                .unwrap_or(false)
        })
        .ok_or(ExtractError::MissingComponent)?;

    // Prefetch the entire info entry in a single HTTP request so the
    // streaming pipeline doesn't trigger many small range requests.
    let entry = &zip_reader.file().entries()[index];
    let offset = entry.header_offset();
    let size = entry.header_size() + entry.compressed_size();
    zip_reader
        .inner_mut()
        .get_mut()
        .get_mut()
        .prefetch(offset..offset + size)
        .await;

    // Get a streaming reader for the ZIP entry (futures::io::AsyncRead).
    // This does NOT buffer the entire entry — bytes are fetched on demand
    // via HTTP range requests as the downstream decompressor/tar reader
    // consumes them.
    let entry_reader = zip_reader.reader_without_entry(index).await?;

    // Pipeline: async ZIP entry reader -> tokio compat -> buffered -> zstd decoder -> tar
    let tokio_reader = entry_reader.compat();
    let buf_reader = tokio::io::BufReader::new(tokio_reader);
    let zstd_decoder = ZstdDecoder::new(buf_reader);
    let mut tar = tokio_tar::Archive::new(zstd_decoder);

    let result = get_file_from_tar_archive(&mut tar, target_path).await?;

    debug!(
        "Requested ranges: {:?}",
        zip_reader
            .inner_mut()
            .get_mut()
            .get_mut()
            .requested_ranges()
            .await
    );

    Ok(result)
}

/// Fetch and parse a typed [`PackageFile`] from a remote `.conda` package
/// using HTTP range requests.
///
/// Only fetches the info section and decompresses only until the target file
/// is found. Returns an error if the server does not support range requests
/// or the package is not a `.conda` file.
///
/// # Example
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// use rattler_conda_types::package::IndexJson;
/// use rattler_package_streaming::reqwest::sparse::fetch_package_file_from_remote_sparse;
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// use url::Url;
///
/// let client = ClientWithMiddleware::from(Client::new());
/// let url = Url::parse("https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda").unwrap();
///
/// let index_json: IndexJson = fetch_package_file_from_remote_sparse(client, url).await.unwrap();
/// println!("Package: {}", index_json.name.as_normalized());
/// # }
/// ```
pub async fn fetch_package_file_from_remote_sparse<P: PackageFile>(
    client: ClientWithMiddleware,
    url: Url,
) -> Result<P, ExtractError> {
    let bytes = fetch_file_from_remote_sparse(client, url, P::package_path())
        .await?
        .ok_or(ExtractError::MissingComponent)?;
    P::from_slice(&bytes)
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
}

/// Stream the full package and extract a single [`PackageFile`].
/// Download the full package and extract a single file from it.
pub async fn fetch_file_from_remote_full_download(
    client: &ClientWithMiddleware,
    url: &Url,
    target_path: &Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
    let archive_type = CondaArchiveType::try_from(std::path::Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?;

    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(ExtractError::ReqwestError)?
        .error_for_status()
        .map_err(|e| ExtractError::ReqwestError(e.into()))?;

    // Convert the response body into an AsyncRead stream (same pattern as reqwest/tokio.rs)
    let byte_stream = response.bytes_stream().map_err(|err| {
        if err.is_body() {
            std::io::Error::new(std::io::ErrorKind::Interrupted, err)
        } else if err.is_decode() {
            std::io::Error::new(std::io::ErrorKind::InvalidData, err)
        } else {
            std::io::Error::other(err)
        }
    });
    let stream_reader = StreamReader::new(byte_stream);

    let file_path = target_path;

    let content = match archive_type {
        CondaArchiveType::TarBz2 => {
            let buf_reader = tokio::io::BufReader::new(stream_reader);
            let decoder = BzDecoder::new(buf_reader);
            let mut archive = tokio_tar::Archive::new(decoder);
            get_file_from_tar_archive(&mut archive, file_path).await?
        }
        CondaArchiveType::Conda => {
            // async_zip uses futures IO traits, so bridge tokio → futures
            let compat_reader = stream_reader.compat();
            let mut buf_reader = futures::io::BufReader::new(compat_reader);
            let mut zip_reader = ZipFileReader::new(&mut buf_reader);

            let mut found: Option<Vec<u8>> = None;
            let prefix = crate::tokio::async_read::conda_entry_prefix(file_path);

            while let Some(mut entry) = zip_reader
                .next_with_entry()
                .await
                .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?
            {
                let filename = entry.reader().entry().filename().as_str().map_err(|e| {
                    ExtractError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;

                if filename.starts_with(prefix) && filename.ends_with(".tar.zst") {
                    // Bridge the entry reader back from futures → tokio
                    let compat_entry = entry.reader_mut().compat();
                    let buf_entry = tokio::io::BufReader::new(compat_entry);
                    let decoder = ZstdDecoder::new(buf_entry);
                    let mut archive = tokio_tar::Archive::new(decoder);
                    found = get_file_from_tar_archive(&mut archive, file_path).await?;
                    break;
                }

                // Skip to the next entry (required by async_zip API)
                (.., zip_reader) = entry
                    .skip()
                    .await
                    .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?;
            }

            found
        }
    };

    Ok(content)
}

/// Stream the full package and extract a single [`PackageFile`].
pub async fn fetch_package_file_from_remote_full_download<P: PackageFile>(
    client: &ClientWithMiddleware,
    url: &Url,
) -> Result<P, ExtractError> {
    let bytes = fetch_file_from_remote_full_download(client, url, P::package_path())
        .await?
        .ok_or(ExtractError::MissingComponent)?;
    P::from_slice(&bytes)
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reqwest::test_server;
    use std::path::PathBuf;

    fn test_file() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-fd-1-0.1.0-h4616a5c_0.conda")
    }

    #[tokio::test]
    async fn test_fetch_package_file_sparse() {
        use rattler_conda_types::package::{AboutJson, IndexJson};

        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let index_json: IndexJson =
            fetch_package_file_from_remote_sparse(client.clone(), url.clone())
                .await
                .unwrap();
        insta::assert_yaml_snapshot!(index_json);

        let about_json: AboutJson = fetch_package_file_from_remote_sparse(client, url)
            .await
            .unwrap();
        insta::assert_yaml_snapshot!(about_json);
    }

    #[tokio::test]
    async fn test_fetch_raw_file() {
        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let raw = fetch_file_from_remote_sparse(client, url, Path::new("info/index.json"))
            .await
            .unwrap()
            .expect("file should exist in archive");
        assert!(!raw.is_empty());
    }

    #[tokio::test]
    async fn test_fetch_pkg_file_sparse() {
        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let raw = fetch_file_from_remote_sparse(client, url, Path::new("clobber"))
            .await
            .unwrap()
            .expect("file should exist in pkg section");
        let content = String::from_utf8(raw).unwrap();
        insta::assert_snapshot!(content, @"clobber-fd-1");
    }
}
