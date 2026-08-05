//! Sparse remote access to files inside `.conda` archives.
//!
//! Thin wrappers around [`crate::archive::PackageArchive`] that require range
//! request support and never fall back to a full download. For fallback
//! behavior see [`super::fetch`]; to read multiple files from one package,
//! use [`crate::archive::PackageArchive`] directly.

use std::path::Path;

use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use rattler_redaction::{DEFAULT_REDACTION_STR, redact_known_secrets_from_url};
use reqwest_middleware::ClientWithMiddleware;
use tracing::instrument;
use url::Url;

use crate::ExtractError;
use crate::archive::PackageArchive;

/// Fetch the raw bytes of a single file from a remote `.conda` package using
/// HTTP range requests. Returns `Ok(None)` if the file is not in the archive.
///
/// Only the bytes needed to reach the target file are downloaded. Errors if
/// the URL is not a `.conda` archive or the server does not support ranges.
#[instrument(skip_all, fields(url = %redact_known_secrets_from_url(&url, DEFAULT_REDACTION_STR).as_ref().unwrap_or(&url), path = %target_path.display()))]
pub async fn fetch_file_from_remote_sparse(
    client: ClientWithMiddleware,
    url: Url,
    target_path: &Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
    if CondaArchiveType::try_from(Path::new(url.path())) != Some(CondaArchiveType::Conda) {
        return Err(ExtractError::UnsupportedArchiveType);
    }
    let archive = PackageArchive::open_sparse(client, url).await?;
    archive.read_file(target_path).await
}

/// Fetch and parse a typed [`PackageFile`] from a remote `.conda` package
/// using HTTP range requests.
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
    if CondaArchiveType::try_from(Path::new(url.path())) != Some(CondaArchiveType::Conda) {
        return Err(ExtractError::UnsupportedArchiveType);
    }
    let archive = PackageArchive::open_sparse(client, url).await?;
    archive.read_package_file().await
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
}
