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

use std::path::Path;

use async_compression::tokio::bufread::ZstdDecoder;
use async_http_range_reader::{AsyncHttpRangeReader, CheckSupportMethod};
use async_zip::base::read::seek::ZipFileReader;
use http::HeaderMap;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::debug;
use url::Url;

use crate::tokio::async_read::get_file_from_tar_archive;
use crate::ExtractError;

/// Default number of bytes to fetch from the end of the file.
/// 64KB should be enough for most packages to include the EOCD, Central Directory,
/// and often the entire info archive.
const DEFAULT_TAIL_SIZE: u64 = 64 * 1024;

/// Fetch the raw bytes of a single file from a remote `.conda` package's info
/// section using HTTP range requests.
///
/// Streams the zstd data directly from the server through an async decompressor
/// and tar reader, stopping as soon as the target file is found. Only the bytes
/// needed to reach the target file are downloaded and decompressed.
///
/// Returns `Ok(None)` if the file is not found in the archive.
/// Returns an error if the URL does not point to a `.conda` archive or the
/// server does not support range requests.
pub async fn fetch_file_from_remote_conda(
    client: ClientWithMiddleware,
    url: Url,
    target_path: &Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
    debug!("fetching {:?} from remote archive {}", target_path, url);

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
    let (index, _) = zip_reader
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

    // Prefetch the entire info entry in a single HTTP request so the
    // streaming pipeline doesn't trigger many small range requests.
    let entry = &zip_reader.file().entries()[index];
    let offset = entry.header_offset();
    let size = entry.header_size() + entry.compressed_size();
    let buffer_size: u64 = 8192;
    let size = size.div_ceil(buffer_size) * buffer_size;

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
    //
    // The pipeline borrows zip_reader, so we scope it in a block to release
    // the borrow before accessing the inner reader for debug logging.
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
    let bytes = fetch_file_from_remote_conda(client, url, P::package_path())
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

        let raw = fetch_file_from_remote_conda(client, url, Path::new("info/index.json"))
            .await
            .unwrap()
            .expect("file should exist in archive");
        assert!(!raw.is_empty());
    }
}
