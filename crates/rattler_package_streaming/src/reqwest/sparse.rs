//! Sparse remote access to files inside `.conda` archives.
//!
//! This module uses HTTP range requests to avoid downloading the full archive.
//! It opens the outer ZIP container, locates the relevant `info-*.tar.zst` or
//! `pkg-*.tar.zst` member, and streams only the bytes needed to read a target
//! path from that inner tarball.
//!
//! Only `.conda` archives on servers that support range requests are supported.
//! For higher-level APIs that fall back to full downloads, see
//! [`super::fetch::fetch_package_file_from_remote_url`] and
//! [`super::fetch::fetch_file_from_remote_url`].
//!
//! # Example
//!
//! ```rust,no_run
//! # #[tokio::main]
//! # async fn main() {
//! use rattler_conda_types::package::IndexJson;
//! use rattler_package_streaming::reqwest::sparse::fetch_package_file_sparse;
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//!
//! let client = ClientWithMiddleware::from(Client::new());
//! let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
//!
//! let index_json: IndexJson = fetch_package_file_sparse(client, url).await.unwrap();
//! # }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_compression::tokio::bufread::ZstdDecoder;
use async_http_range_reader::{AsyncHttpRangeReader, CheckSupportMethod};
use async_zip::base::read::seek::ZipFileReader;
use http::HeaderMap;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use rattler_redaction::{redact_known_secrets_from_url, DEFAULT_REDACTION_STR};
use reqwest_middleware::ClientWithMiddleware;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::{debug, instrument};
use url::Url;

use crate::tokio::async_read::{conda_entry_prefix, get_files_from_tar_archive};
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
    match fetch_files_from_remote_sparse(client, url, [target_path.to_path_buf()]).await {
        Ok(mut files) => Ok(files.pop().map(|(_, bytes)| bytes)),
        Err(ExtractError::MissingPaths { .. }) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Fetch the raw bytes for multiple files from a remote `.conda` package using
/// HTTP range requests.
///
/// Streams each required inner tarball at most once and returns the files in the
/// first-seen order of `target_paths`.
pub async fn fetch_files_from_remote_sparse<I>(
    client: ClientWithMiddleware,
    url: Url,
    target_paths: I,
) -> Result<Vec<(PathBuf, Vec<u8>)>, ExtractError>
where
    I: IntoIterator<Item = PathBuf>,
{
    let archive_type = CondaArchiveType::try_from(std::path::Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?;

    if archive_type != CondaArchiveType::Conda {
        return Err(ExtractError::UnsupportedArchiveType);
    }

    let target_paths: Vec<PathBuf> = target_paths.into_iter().collect();
    if target_paths.is_empty() {
        return Ok(Vec::new());
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

    let mut grouped_paths: HashMap<&'static str, Vec<PathBuf>> = HashMap::new();
    for path in &target_paths {
        grouped_paths
            .entry(conda_entry_prefix(path))
            .or_default()
            .push(path.clone());
    }

    let mut found_files = HashMap::with_capacity(target_paths.len());
    for prefix in ["info-", "pkg-"] {
        let Some(component_paths) = grouped_paths.get(prefix) else {
            continue;
        };

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

        // Prefetch the entire tar.zst entry in a single HTTP request so the
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

        let entry_reader = zip_reader.reader_without_entry(index).await?;
        let tokio_reader = entry_reader.compat();
        let buf_reader = tokio::io::BufReader::new(tokio_reader);
        let zstd_decoder = ZstdDecoder::new(buf_reader);
        let mut tar = tokio_tar::Archive::new(zstd_decoder);

        for (path, bytes) in get_files_from_tar_archive(&mut tar, component_paths).await? {
            found_files.insert(path, bytes);
        }
    }

    debug!(
        "Requested ranges: {:?}",
        zip_reader
            .inner_mut()
            .get_mut()
            .get_mut()
            .requested_ranges()
            .await
    );

    let missing_paths = target_paths
        .iter()
        .filter(|path| !found_files.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_paths.is_empty() {
        return Err(ExtractError::MissingPaths {
            paths: missing_paths,
        });
    }

    Ok(target_paths
        .into_iter()
        .filter_map(|path| found_files.remove(&path).map(|bytes| (path, bytes)))
        .collect())
}

/// Fetch and parse a typed [`PackageFile`] from a remote `.conda` package
/// using HTTP range requests.
///
/// This is a thin typed wrapper around [`fetch_file_from_remote_sparse`]. It
/// only works for `.conda` archives on servers that support range requests and
/// does not perform any full-download fallback.
///
/// # Example
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// use rattler_conda_types::package::IndexJson;
/// use rattler_package_streaming::reqwest::sparse::fetch_package_file_sparse;
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// use url::Url;
///
/// let client = ClientWithMiddleware::from(Client::new());
/// let url = Url::parse("https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda").unwrap();
///
/// let index_json: IndexJson = fetch_package_file_sparse(client, url).await.unwrap();
/// println!("Package: {}", index_json.name.as_normalized());
/// # }
/// ```
pub async fn fetch_package_file_sparse<P: PackageFile>(
    client: ClientWithMiddleware,
    url: Url,
) -> Result<P, ExtractError> {
    let bytes = fetch_files_from_remote_sparse(client, url, [P::package_path().to_path_buf()])
        .await?
        .into_iter()
        .next()
        .map(|(_, bytes)| bytes)
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

        let index_json: IndexJson = fetch_package_file_sparse(client.clone(), url.clone())
            .await
            .unwrap();
        insta::assert_yaml_snapshot!(index_json);

        let about_json: AboutJson = fetch_package_file_sparse(client, url).await.unwrap();
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

    #[tokio::test]
    async fn test_fetch_multiple_files_sparse() {
        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let files = fetch_files_from_remote_sparse(
            client,
            url,
            [
                PathBuf::from("info/index.json"),
                PathBuf::from("info/about.json"),
                PathBuf::from("clobber"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(files.len(), 3);
        assert_eq!(files[0].0, PathBuf::from("info/index.json"));
        assert_eq!(files[1].0, PathBuf::from("info/about.json"));
        assert_eq!(files[2].0, PathBuf::from("clobber"));
        assert!(files.iter().all(|(_, bytes)| !bytes.is_empty()));
    }
}
