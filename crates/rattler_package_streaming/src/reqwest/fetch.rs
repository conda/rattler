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
use crate::reqwest::sparse::fetch_package_file_from_remote_full_download;
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

    fetch_package_file_from_remote_full_download::<P>(&client, &url).await
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
