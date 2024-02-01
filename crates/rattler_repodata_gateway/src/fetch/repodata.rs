//! This module provides functionality to download and cache `repodata.json` from a remote location.

use std::path::PathBuf;
use tracing::instrument;
use url::Url;

use super::{CachedData, FetchError, Options, ProgressFunc, Variant};

/// dpcs
pub type FetchRepoDataOptions = Options<RepoDataVariant>;

/// Defines which type of repodata.json file to download. Usually you want to use the
/// [`Variant::AfterPatches`] variant because that reflects the repodata with any patches applied.
#[derive(Default, Copy, Clone, Debug, PartialEq, Eq)]
pub enum RepoDataVariant {
    /// Fetch the `repodata.json` file. This `repodata.json` has repodata patches applied. Packages
    /// may have also been removed from this file (yanked).
    #[default]
    AfterPatches,

    /// Fetch the `repodata_from_packages.json` file. This file contains all packages with the
    /// information extracted from their index.json file. This file is not patched and contains all
    /// packages ever uploaded.
    ///
    /// Note that this file is not available for all channels. This only seems to be available for
    /// the conda-forge and bioconda channels on anaconda.org.
    FromPackages,

    /// Fetch `current_repodata.json` file. This file contains only the latest version of each
    /// package.
    ///
    /// Note that this file is not available for all channels. This only seems to be available for
    /// the conda-forge and bioconda channels on anaconda.org.
    Current,
}

impl Variant for RepoDataVariant {
    fn file_name(&self) -> &'static str {
        match self {
            RepoDataVariant::AfterPatches => "repodata.json",
            RepoDataVariant::FromPackages => "repodata_from_packages.json",
            RepoDataVariant::Current => "current_repodata.json",
        }
    }
}

/// Fetch the repodata.json file for the given subdirectory. The result is cached on disk using the
/// HTTP cache headers returned from the server.
///
/// The successful result of this function also returns a lockfile which ensures that both the state
/// and the repodata that is pointed to remain in sync. However, not releasing the lockfile (by
/// dropping it) could block other threads and processes, it is therefore advisable to release it as
/// quickly as possible.
///
/// This method implements several different methods to download the repodata.json file from the
/// remote:
///
/// * If a `repodata.json.zst` file is available in the same directory that file is downloaded
///   and decompressed.
/// * If a `repodata.json.bz2` file is available in the same directory that file is downloaded
///   and decompressed.
/// * Otherwise the regular `repodata.json` file is downloaded.
///
/// The checks to see if a `.zst` and/or `.bz2` file exist are performed by doing a HEAD request to
/// the respective URLs. The result of these are cached.
#[instrument(err, skip_all, fields(subdir_url, cache_path = %cache_path.display()))]
pub async fn fetch_repo_data(
    subdir_url: Url,
    client: reqwest_middleware::ClientWithMiddleware,
    cache_path: PathBuf,
    options: FetchRepoDataOptions,
    progress: Option<ProgressFunc>,
) -> Result<CachedData, FetchError> {
    super::_fetch_data(subdir_url, client, cache_path, options, progress).await
}

#[cfg(test)]
mod test {
    use super::super::{
        fetch_repo_data, normalize_subdir_url, CacheResult, CachedData, DownloadProgress, Options,
    };
    use crate::fetch::cache::Expiring;
    use crate::fetch::{DataNotFoundError, FetchError};
    use crate::utils::simple_channel_server::SimpleChannelServer;
    use crate::utils::Encoding;
    use assert_matches::assert_matches;
    use hex_literal::hex;
    use rattler_networking::AuthenticationMiddleware;
    use reqwest::Client;
    use reqwest_middleware::ClientWithMiddleware;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;
    use url::Url;

    async fn write_encoded(
        mut input: &[u8],
        destination: &Path,
        encoding: Encoding,
    ) -> Result<(), std::io::Error> {
        // Open the file for writing
        let mut file = tokio::fs::File::create(destination).await.unwrap();

        match encoding {
            Encoding::Passthrough => {
                tokio::io::copy(&mut input, &mut file).await?;
            }
            Encoding::GZip => {
                let mut encoder = async_compression::tokio::write::GzipEncoder::new(file);
                tokio::io::copy(&mut input, &mut encoder).await?;
                encoder.shutdown().await?;
            }
            Encoding::Bz2 => {
                let mut encoder = async_compression::tokio::write::BzEncoder::new(file);
                tokio::io::copy(&mut input, &mut encoder).await?;
                encoder.shutdown().await?;
            }
            Encoding::Zst => {
                let mut encoder = async_compression::tokio::write::ZstdEncoder::new(file);
                tokio::io::copy(&mut input, &mut encoder).await?;
                encoder.shutdown().await?;
            }
        }

        Ok(())
    }

    #[test]
    pub fn test_normalize_url() {
        assert_eq!(
            normalize_subdir_url(Url::parse("http://localhost/channels/empty").unwrap()),
            Url::parse("http://localhost/channels/empty/").unwrap(),
        );
        assert_eq!(
            normalize_subdir_url(Url::parse("http://localhost/channels/empty/").unwrap()),
            Url::parse("http://localhost/channels/empty/").unwrap(),
        );
    }

    const FAKE_REPO_DATA: &str = r#"{
        "packages.conda": {
            "asttokens-2.2.1-pyhd8ed1ab_0.conda": {
                "arch": null,
                "build": "pyhd8ed1ab_0",
                "build_number": 0,
                "build_string": "pyhd8ed1ab_0",
                "constrains": [],
                "depends": [
                    "python >=3.5",
                    "six"
                ],
                "fn": "asttokens-2.2.1-pyhd8ed1ab_0.conda",
                "license": "Apache-2.0",
                "license_family": "Apache",
                "md5": "bf7f54dd0f25c3f06ecb82a07341841a",
                "name": "asttokens",
                "noarch": "python",
                "platform": null,
                "sha256": "7ed530efddd47a96c11197906b4008405b90e3bc2f4e0df722a36e0e6103fd9c",
                "size": 27831,
                "subdir": "noarch",
                "timestamp": 1670264089059,
                "track_features": "",
                "url": "https://conda.anaconda.org/conda-forge/noarch/asttokens-2.2.1-pyhd8ed1ab_0.conda",
                "version": "2.2.1"
            }
        }
    }
    "#;

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_fetch_repo_data() {
        // Create a directory with some repodata.
        let subdir_path = TempDir::new().unwrap();
        std::fs::write(subdir_path.path().join("repodata.json"), FAKE_REPO_DATA).unwrap();
        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_repo_data(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            result.cache_state.blake2_hash.unwrap()[..],
            hex!("a1861e448e4a62b88dce47c95351bfbe7fc22451a73f89a09d782492540e0675")[..]
        );
        assert_eq!(
            std::fs::read_to_string(result.path).unwrap(),
            FAKE_REPO_DATA
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_cache_works() {
        // Create a directory with some repodata.
        let subdir_path = TempDir::new().unwrap();
        std::fs::write(subdir_path.path().join("repodata.json"), FAKE_REPO_DATA).unwrap();
        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let CachedData { cache_result, .. } = fetch_repo_data(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.path().to_owned(),
            Options::default(),
            None,
        )
        .await
        .unwrap();

        assert_matches!(cache_result, CacheResult::CacheNotPresent);

        // Download the data from the channel with a filled cache.
        let CachedData { cache_result, .. } = fetch_repo_data(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.path().to_owned(),
            Options::default(),
            None,
        )
        .await
        .unwrap();

        assert_matches!(
            cache_result,
            CacheResult::CacheHit | CacheResult::CacheHitAfterFetch
        );

        // I know this is terrible but without the sleep rust is too blazingly fast and the server
        // doesnt think the file was actually updated.. This is because the time send by the server
        // has seconds precision.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        // Update the original repodata.json file
        std::fs::write(subdir_path.path().join("repodata.json"), FAKE_REPO_DATA).unwrap();

        // Download the data from the channel with a filled cache.
        let CachedData { cache_result, .. } = fetch_repo_data(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            None,
        )
        .await
        .unwrap();

        assert_matches!(cache_result, CacheResult::CacheOutdated);
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_zst_works() {
        let subdir_path = TempDir::new().unwrap();
        write_encoded(
            FAKE_REPO_DATA.as_bytes(),
            &subdir_path.path().join("repodata.json.zst"),
            Encoding::Zst,
        )
        .await
        .unwrap();

        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_repo_data(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(result.path).unwrap(),
            FAKE_REPO_DATA
        );
        assert_matches!(
            result.cache_state.has_zst, Some(Expiring {
                value, ..
            }) if value
        );
        assert_matches!(
            result.cache_state.has_bz2, Some(Expiring {
                value, ..
            }) if !value
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_bz2_works() {
        let subdir_path = TempDir::new().unwrap();
        write_encoded(
            FAKE_REPO_DATA.as_bytes(),
            &subdir_path.path().join("repodata.json.bz2"),
            Encoding::Bz2,
        )
        .await
        .unwrap();

        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_repo_data(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(result.path).unwrap(),
            FAKE_REPO_DATA
        );
        assert_matches!(
            result.cache_state.has_zst, Some(Expiring {
                value, ..
            }) if !value
        );
        assert_matches!(
            result.cache_state.has_bz2, Some(Expiring {
                value, ..
            }) if value
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_zst_is_preferred() {
        let subdir_path = TempDir::new().unwrap();
        write_encoded(
            FAKE_REPO_DATA.as_bytes(),
            &subdir_path.path().join("repodata.json.bz2"),
            Encoding::Bz2,
        )
        .await
        .unwrap();
        write_encoded(
            FAKE_REPO_DATA.as_bytes(),
            &subdir_path.path().join("repodata.json.zst"),
            Encoding::Zst,
        )
        .await
        .unwrap();

        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_repo_data(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(result.path).unwrap(),
            FAKE_REPO_DATA
        );
        assert!(result.cache_state.url.path().ends_with("repodata.json.zst"));
        assert_matches!(
            result.cache_state.has_zst, Some(Expiring {
                value, ..
            }) if value
        );
        assert_matches!(
            result.cache_state.has_bz2, Some(Expiring {
                value, ..
            }) if value
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_gzip_transfer_encoding() {
        // Create a directory with some repodata.
        let subdir_path = TempDir::new().unwrap();
        write_encoded(
            FAKE_REPO_DATA.as_ref(),
            &subdir_path.path().join("repodata.json.gz"),
            Encoding::GZip,
        )
        .await
        .unwrap();

        // The server is configured in such a way that if file `a` is requested but a file called
        // `a.gz` is available it will stream the `a.gz` file and report that its a `gzip` encoded
        // stream.
        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel
        let cache_dir = TempDir::new().unwrap();

        let client = Client::builder().no_gzip().build().unwrap();
        let authenticated_client = reqwest_middleware::ClientBuilder::new(client)
            .with_arc(Arc::new(AuthenticationMiddleware::default()))
            .build();

        let result = fetch_repo_data(
            server.url(),
            authenticated_client,
            cache_dir.into_path(),
            Options::default(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(result.path).unwrap(),
            FAKE_REPO_DATA
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_progress() {
        // Create a directory with some repodata.
        let subdir_path = TempDir::new().unwrap();
        std::fs::write(subdir_path.path().join("repodata.json"), FAKE_REPO_DATA).unwrap();
        let server = SimpleChannelServer::new(subdir_path.path());

        let last_download_progress = Arc::new(AtomicU64::new(0));
        let last_download_progress_captured = last_download_progress.clone();
        let download_progress = move |progress: DownloadProgress| {
            last_download_progress_captured.store(progress.bytes, Ordering::SeqCst);
            assert_eq!(progress.total, Some(1110));
        };

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let _result = fetch_repo_data(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            Some(Box::new(download_progress)),
        )
        .await
        .unwrap();

        assert_eq!(last_download_progress.load(Ordering::SeqCst), 1110);
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_repodata_not_found() {
        // Create a directory with some repodata.
        let subdir_path = TempDir::new().unwrap();
        // Don't add repodata to the channel.

        // Download the "data" from the local filebased channel.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_repo_data(
            Url::parse(format!("file://{}", subdir_path.path().to_str().unwrap()).as_str())
                .unwrap(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            None,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(FetchError::NotFound(DataNotFoundError::FileSystemError(_)))
        ));

        // Start a server to test the http error
        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the "data" from the channel.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_repo_data(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            None,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(FetchError::NotFound(DataNotFoundError::HttpError(_)))
        ));
    }
}
