//! This module provides functionality to download and cache `run_exports.json` from a remote location.

use std::path::PathBuf;
use url::Url;

use super::{CacheAction, CachedData, FetchError, Options, ProgressFunc, Variant, _fetch_data};

#[derive(Default)]
/// run_export variants
pub struct RunExportsVariants;
impl Variant for RunExportsVariants {
    fn file_name(&self) -> &'static str {
        "run_exports.json"
    }
}

impl Default for Options<RunExportsVariants> {
    fn default() -> Self {
        Self {
            cache_action: CacheAction::default(),
            variant: RunExportsVariants::default(),
            jlap_enabled: false,
            zstd_enabled: true,
            bz2_enabled: true,
        }
    }
}

/// Fetch a data file for the given channel platform url.
/// The result is cached on disk using the HTTP cache headers returned from the server.
///
/// The successful result of this function also returns a lockfile which ensures that both the state
/// and the run_exports that is pointed to remain in sync. However, not releasing the lockfile (by
/// dropping it) could block other threads and processes, it is therefore advisable to release it as
/// quickly as possible.
///
/// This method implements several different methods to download the run_exports.json file from the
/// remote:
///
/// * If a `file.json.zst` file is available in the same directory that file is downloaded
///   and decompressed.
/// * If a `file.json.bz2` file is available in the same directory that file is downloaded
///   and decompressed.
/// * Otherwise the regular `file.json` file is downloaded.
///
/// The checks to see if a `.zst` and/or `.bz2` file exist are performed by doing a HEAD request to
/// the respective URLs. The result of these are cached.
pub async fn fetch_run_exports(
    channel_platform_url: Url,
    client: reqwest_middleware::ClientWithMiddleware,
    cache_path: PathBuf,
    options: Options<RunExportsVariants>,
    progress: Option<ProgressFunc>,
) -> Result<CachedData, FetchError> {
    _fetch_data(channel_platform_url, client, cache_path, options, progress).await
}

#[cfg(test)]
mod test {
    use super::{
        super::{normalize_subdir_url, CacheResult, DownloadProgress, Expiring, Options},
        fetch_run_exports, CachedData,
    };
    use crate::fetch::{DataNotFoundError, FetchError};
    use crate::utils::simple_channel_server::SimpleChannelServer;
    use crate::utils::Encoding;
    use assert_matches::assert_matches;
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

    const FAKE_RUN_EXPORTS: &str = r#"{
        "packages.conda": {
            "cross-python_osx-64-3.10-41_cpython.conda": {
                "run_exports": {
                    "strong": [ "python 3.10.* *_cpython" ],
                    "weak": [ "libzlib >=1.2.13,<1.3.0a0" ]
                }
            }
        }
    }
    "#;

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_fetch_run_exports() {
        // Create a directory with some run_exports.
        let subdir_path = TempDir::new().unwrap();
        std::fs::write(
            subdir_path.path().join("run_exports.json"),
            FAKE_RUN_EXPORTS,
        )
        .unwrap();
        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_run_exports(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            hex::encode(result.cache_state.blake2_hash.unwrap()),
            "be96ff041feb630cc2f40412cb52100507269876254bd2cda92203dd6bf3a82c"
        );
        assert_eq!(
            std::fs::read_to_string(result.path).unwrap(),
            FAKE_RUN_EXPORTS
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_cache_works() {
        // Create a directory with some run_exports.
        let subdir_path = TempDir::new().unwrap();
        std::fs::write(
            subdir_path.path().join("run_exports.json"),
            FAKE_RUN_EXPORTS,
        )
        .unwrap();
        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let CachedData { cache_result, .. } = fetch_run_exports(
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
        let CachedData { cache_result, .. } = fetch_run_exports(
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

        // Update the original run_exports.json file
        std::fs::write(
            subdir_path.path().join("run_exports.json"),
            FAKE_RUN_EXPORTS,
        )
        .unwrap();

        // Download the data from the channel with a filled cache.
        let CachedData { cache_result, .. } = fetch_run_exports(
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
            FAKE_RUN_EXPORTS.as_bytes(),
            &subdir_path.path().join("run_exports.json.zst"),
            Encoding::Zst,
        )
        .await
        .unwrap();

        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_run_exports(
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
            FAKE_RUN_EXPORTS
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
            FAKE_RUN_EXPORTS.as_bytes(),
            &subdir_path.path().join("run_exports.json.bz2"),
            Encoding::Bz2,
        )
        .await
        .unwrap();

        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_run_exports(
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
            FAKE_RUN_EXPORTS
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
            FAKE_RUN_EXPORTS.as_bytes(),
            &subdir_path.path().join("run_exports.json.bz2"),
            Encoding::Bz2,
        )
        .await
        .unwrap();
        write_encoded(
            FAKE_RUN_EXPORTS.as_bytes(),
            &subdir_path.path().join("run_exports.json.zst"),
            Encoding::Zst,
        )
        .await
        .unwrap();

        let server = SimpleChannelServer::new(subdir_path.path());

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_run_exports(
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
            FAKE_RUN_EXPORTS
        );
        assert!(result
            .cache_state
            .url
            .path()
            .ends_with("run_exports.json.zst"));
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
        // Create a directory with some run_exports.
        let subdir_path = TempDir::new().unwrap();
        write_encoded(
            FAKE_RUN_EXPORTS.as_ref(),
            &subdir_path.path().join("run_exports.json.gz"),
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

        let result = fetch_run_exports(
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
            FAKE_RUN_EXPORTS
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_progress() {
        // Create a directory with some run_exports.
        let subdir_path = TempDir::new().unwrap();
        std::fs::write(
            subdir_path.path().join("run_exports.json"),
            FAKE_RUN_EXPORTS,
        )
        .unwrap();
        let server = SimpleChannelServer::new(subdir_path.path());

        let last_download_progress = Arc::new(AtomicU64::new(0));
        let last_download_progress_captured = last_download_progress.clone();
        let download_progress = move |progress: DownloadProgress| {
            last_download_progress_captured.store(progress.bytes, Ordering::SeqCst);
            assert_eq!(progress.total, Some(295));
        };

        // Download the data from the channel with an empty cache.
        let cache_dir = TempDir::new().unwrap();
        let _result = fetch_run_exports(
            server.url(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.into_path(),
            Options::default(),
            Some(Box::new(download_progress)),
        )
        .await
        .unwrap();

        assert_eq!(last_download_progress.load(Ordering::SeqCst), 295);
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_run_exports_not_found() {
        // Create a directory with some run_exports.
        let subdir_path = TempDir::new().unwrap();
        // Don't add run_exports to the channel.

        // Download the "data" from the local filebased channel.
        let cache_dir = TempDir::new().unwrap();
        let result = fetch_run_exports(
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
        let result = fetch_run_exports(
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
