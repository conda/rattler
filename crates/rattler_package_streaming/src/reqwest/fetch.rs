//! High-level helpers to fetch files from remote Conda packages.
//!
//! These helpers first try the sparse HTTP range-request path from [`super::sparse`]
//! and automatically fall back to streaming the full archive through
//! [`super::full_download`] when range requests are unsupported or the archive type
//! cannot be handled sparsely.
//!
//! Use this module when you want a single entry point that works for both typed
//! [`PackageFile`] members and arbitrary file paths inside `.conda` or `.tar.bz2`
//! packages.
//!
//! # Example
//!
//! ```rust,no_run
//! # #[tokio::main]
//! # async fn main() {
//! use rattler_conda_types::package::IndexJson;
//! use rattler_package_streaming::reqwest::fetch::fetch_package_file_from_remote_url;
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//!
//! let client = ClientWithMiddleware::from(Client::new());
//! let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
//!
//! let index_json: IndexJson = fetch_package_file_from_remote_url(client, url)
//!     .await
//!     .unwrap();
//!
//! println!("Package: {}", index_json.name.as_normalized());
//! # }
//! ```

use async_http_range_reader::AsyncHttpRangeReaderError;
use rattler_conda_types::package::PackageFile;
use reqwest_middleware::ClientWithMiddleware;
use tracing::debug;
use url::Url;

pub use super::full_download::{
    fetch_file_from_remote_full_download, fetch_package_file_full_download,
};
use super::sparse::fetch_package_file_sparse;
use crate::reqwest::sparse::fetch_file_from_remote_sparse;
use crate::ExtractError;

/// Fetch and parse a specific [`PackageFile`] from a remote package.
///
/// The function first attempts the sparse range-request path, which usually only
/// downloads the bytes needed to reach the requested file inside a `.conda`
/// archive.
///
/// If the server does not support range requests, or if the archive type cannot
/// be handled by the sparse implementation, it falls back to the streaming
/// full-download path.
///
/// For lower-level access, see [`super::sparse::fetch_file_from_remote_sparse`]
/// and [`super::full_download::fetch_file_from_remote_full_download`].
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
/// use rattler_package_streaming::reqwest::fetch::fetch_package_file_from_remote_url;
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// use url::Url;
///
/// let client = ClientWithMiddleware::from(Client::new());
/// let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
///
/// let index_json: IndexJson = fetch_package_file_from_remote_url(client, url)
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn fetch_package_file_from_remote_url<P: PackageFile>(
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
        Err(ExtractError::AsyncHttpRangeReaderError(AsyncHttpRangeReaderError::HttpError(err)))
            if err.status() == Some(reqwest::StatusCode::RANGE_NOT_SATISFIABLE) =>
        {
            // this can happen with JFrog Artifactory when you query more than the object length
            debug!("server returned range not satisfiable, falling back to full download");
        }
        Err(ExtractError::AsyncZipError(err)) => {
            // The seek-based sparse reader parses the ZIP central directory, which
            // can fail on archives created by tools (e.g. conda-build / Python's
            // zipfile) that include zip64 extended information fields even when file
            // sizes fit in 32 bits.  The streaming full-download path only reads
            // local file headers and is not affected.
            // See: https://github.com/conda/rattler/issues/2305
            debug!("sparse zip parsing failed ({err}), falling back to full download");
        }
        Err(e) => return Err(e),
    }

    fetch_package_file_full_download::<P>(&client, &url).await
}

/// Fetch the raw bytes for an arbitrary file path inside a remote package.
///
/// The function first attempts the sparse range-request path for `.conda`
/// packages and falls back to streaming the full archive when sparse access is
/// unavailable.
///
/// Returns `Ok(None)` when the target path does not exist in the archive.
pub async fn fetch_file_from_remote_url(
    client: ClientWithMiddleware,
    url: Url,
    target_path: &std::path::Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
    match fetch_file_from_remote_sparse(client.clone(), url.clone(), target_path).await {
        Ok(result) => return Ok(result),
        Err(ExtractError::UnsupportedArchiveType) => {
            debug!("archive type not supported for range requests, falling back to full download");
        }
        Err(ExtractError::AsyncHttpRangeReaderError(
            AsyncHttpRangeReaderError::HttpRangeRequestUnsupported,
        )) => {
            debug!("server does not support range requests, falling back to full download");
        }
        Err(ExtractError::AsyncHttpRangeReaderError(AsyncHttpRangeReaderError::HttpError(err)))
            if err.status() == Some(reqwest::StatusCode::RANGE_NOT_SATISFIABLE) =>
        {
            // this can happen with JFrog Artifactory when you query more than the object length
            debug!("server returned range not satisfiable, falling back to full download");
        }
        Err(ExtractError::AsyncZipError(err)) => {
            // See the matching arm in `fetch_package_file_from_remote_url` above.
            debug!("sparse zip parsing failed ({err}), falling back to full download");
        }
        Err(e) => return Err(e),
    }

    fetch_file_from_remote_full_download(&client, &url, target_path).await
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

        let index_json: IndexJson = fetch_package_file_from_remote_url(client, url)
            .await
            .unwrap();

        insta::assert_yaml_snapshot!(index_json);
    }

    #[tokio::test]
    async fn test_fetch_about_json() {
        use rattler_conda_types::package::AboutJson;

        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let about_json: AboutJson = fetch_package_file_from_remote_url(client, url)
            .await
            .unwrap();

        insta::assert_yaml_snapshot!(about_json);
    }

    /// tar.bz2 is unsupported by the sparse path, so `fetch_package_file_from_remote_url`
    /// falls through to `fetch_package_file_full_download` (streaming).
    #[tokio::test]
    async fn test_fetch_full_download_tar_bz2() {
        use rattler_conda_types::package::IndexJson;

        let tar_bz2 = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2");
        let url = test_server::serve_file(tar_bz2).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let index_json: IndexJson = fetch_package_file_from_remote_url(client, url)
            .await
            .unwrap();

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

    #[tokio::test]
    async fn test_fetch_file_from_remote() {
        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let raw = fetch_file_from_remote_url(client, url, std::path::Path::new("info/index.json"))
            .await
            .unwrap()
            .expect("file should exist in archive");

        assert!(!raw.is_empty());
    }

    #[tokio::test]
    async fn test_fetch_file_from_remote_tar_bz2_fallback() {
        let tar_bz2 = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2");
        let url = test_server::serve_file(tar_bz2).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let raw = fetch_file_from_remote_url(client, url, std::path::Path::new("info/index.json"))
            .await
            .unwrap()
            .expect("file should exist in archive");

        assert!(!raw.is_empty());
    }

    /// Regression test for <https://github.com/conda/rattler/issues/2305>.
    ///
    /// Archives created by conda-build (Python's `zipfile`) include zip64
    /// extended information extra fields in the central directory even when
    /// file sizes fit in 32 bits.  The seek-based sparse reader rejects
    /// these, so `fetch_package_file_from_remote_url` must fall back to the
    /// streaming full-download path.
    #[tokio::test]
    async fn test_fetch_zip64_extra_field_fallback() {
        use rattler_conda_types::package::IndexJson;

        let zip64_pkg = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/zip64-extra-field-test.conda");
        let url = test_server::serve_file(zip64_pkg).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let index_json: IndexJson = fetch_package_file_from_remote_url(client, url)
            .await
            .unwrap();

        assert_eq!(index_json.name.as_normalized(), "zip64-test-pkg");
        assert_eq!(index_json.version.to_string(), "1.0");
    }

    /// Same as above but for the raw-bytes API.
    #[tokio::test]
    async fn test_fetch_file_zip64_extra_field_fallback() {
        let zip64_pkg = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/zip64-extra-field-test.conda");
        let url = test_server::serve_file(zip64_pkg).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let raw = fetch_file_from_remote_url(client, url, std::path::Path::new("info/index.json"))
            .await
            .unwrap()
            .expect("file should exist in archive");

        let parsed: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(parsed["name"], "zip64-test-pkg");
    }
}
