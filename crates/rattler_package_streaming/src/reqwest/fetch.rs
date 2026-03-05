//! Functionality to fetch package metadata from a remote `.conda` package using HTTP range requests.
//!
//! This module allows fetching just the `info/` section of a `.conda` package without downloading
//! the entire file. This is achieved by using HTTP Range requests to fetch only the necessary
//! bytes from the end of the zip archive.
//!
//! For lower-level access, see [`super::sparse`] which exposes the raw-bytes API.
//!
//! # Example
//!
//! ```rust,no_run
//! # #[tokio::main]
//! # async fn main() {
//! use rattler_conda_types::package::IndexJson;
//! use rattler_package_streaming::reqwest::fetch::fetch_package_file_from_url;
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

use async_compression::tokio::bufread::{BzDecoder, ZstdDecoder};
use async_http_range_reader::AsyncHttpRangeReaderError;
use async_zip::base::read::stream::ZipFileReader;
use futures_util::stream::TryStreamExt;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tokio_util::io::StreamReader;
use tracing::debug;
use url::Url;

use super::sparse::fetch_package_file_sparse;
use crate::tokio::async_read::get_file_from_tar_archive;
use crate::ExtractError;

/// Fetch a specific [`PackageFile`] from a remote package using HTTP range requests.
///
/// This function fetches only the minimal bytes needed from the package, typically
/// just the `info/` section which is located at the end of the `.conda` archive.
///
/// If the server does not support range requests or the package is not a `.conda` file,
/// the function falls back to downloading the entire package.
///
/// For lower-level access, see [`super::sparse::fetch_file_from_remote_conda`]
/// which returns raw bytes for a specific file path.
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
/// use rattler_package_streaming::reqwest::fetch::fetch_package_file_from_url;
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
    match fetch_package_file_sparse::<P>(client.clone(), url.clone()).await {
        Ok(result) => return Ok(result),
        Err(ExtractError::UnsupportedArchiveType) => {
            debug!("archive type not supported for range requests, falling back to full download");
        }
        Err(ExtractError::AsyncHttpRangeReaderError(
            AsyncHttpRangeReaderError::HttpRangeRequestUnsupported,
        )) => {
            debug!("server does not support range requests, falling back to full download");
        }
        Err(e) => return Err(e),
    }

    fetch_package_file_full_download::<P>(&client, &url).await
}

/// Stream the full package and extract a single [`PackageFile`].
async fn fetch_package_file_full_download<P: PackageFile>(
    client: &ClientWithMiddleware,
    url: &Url,
) -> Result<P, ExtractError> {
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

    let file_path = std::path::Path::new(P::package_path());

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

            while let Some(mut entry) = zip_reader
                .next_with_entry()
                .await
                .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?
            {
                let filename = entry.reader().entry().filename().as_str().map_err(|e| {
                    ExtractError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;

                if filename.starts_with("info-") && filename.ends_with(".tar.zst") {
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

    let content = content.ok_or_else(|| {
        ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("'{}' not found in archive", P::package_path().display()),
        ))
    })?;

    P::from_slice(&content)
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reqwest::test_server;

    fn test_file() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-fd-1-0.1.0-h4616a5c_0.conda")
    }

    #[tokio::test]
    async fn test_fetch_index_json() {
        use rattler_conda_types::package::IndexJson;

        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let index_json: IndexJson = fetch_package_file_from_url(client, url).await.unwrap();

        insta::assert_yaml_snapshot!(index_json);
    }

    #[tokio::test]
    async fn test_fetch_about_json() {
        use rattler_conda_types::package::AboutJson;

        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let about_json: AboutJson = fetch_package_file_from_url(client, url).await.unwrap();

        insta::assert_yaml_snapshot!(about_json);
    }

    /// tar.bz2 is unsupported by the sparse path, so `fetch_package_file_from_url`
    /// falls through to `fetch_package_file_full_download` (streaming).
    #[tokio::test]
    async fn test_fetch_full_download_tar_bz2() {
        use rattler_conda_types::package::IndexJson;

        let tar_bz2 = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2");
        let url = test_server::serve_file(tar_bz2).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let index_json: IndexJson = fetch_package_file_from_url(client, url).await.unwrap();

        insta::assert_yaml_snapshot!(index_json);
    }

    /// Exercise the streaming `.conda` full-download path directly.
    #[tokio::test]
    async fn test_fetch_full_download_conda() {
        use rattler_conda_types::package::IndexJson;

        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let index_json: IndexJson = fetch_package_file_full_download(&client, &url)
            .await
            .unwrap();

        insta::assert_yaml_snapshot!(index_json);
    }
}
