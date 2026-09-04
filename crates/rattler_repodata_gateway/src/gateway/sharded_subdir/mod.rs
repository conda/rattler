use std::borrow::Cow;
use std::sync::Arc;

use cfg_if::cfg_if;
use http::StatusCode;
use rattler_conda_types::{
    ChannelUrl, RepoDataRecord, Shard, UrlOrPath, WhlPackageRecord,
    package::{CondaArchiveType, DistArchiveIdentifier, WheelArchiveType},
};
use rattler_redaction::Redact;
use url::Url;

use crate::{
    GatewayError,
    fetch::FetchRepoDataError,
    gateway::subdir::{PackageRecords, extract_unique_deps_split},
};

/// Returns `true` if the HTTP status indicates that the server does not expose
/// sharded repodata. We treat 404 (Not Found) and 501 (Not Implemented) the
/// same: the resource is unavailable and we should fall back to repodata.json.
pub(super) fn is_missing_sharded_repodata_status(status: StatusCode) -> bool {
    status == StatusCode::NOT_FOUND || status == StatusCode::NOT_IMPLEMENTED
}

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        mod wasm;
        pub use wasm::ShardedSubdir;
    } else {
        mod tokio;
        pub use tokio::{ShardCachePolicy, ShardedSubdir};
        // Re-exported for use in tests
        #[cfg(test)]
        pub(crate) use tokio::{REPODATA_SHARDS_FILENAME, SHARDS_CACHE_SUFFIX};
    }
}

/// Returns the URL with a trailing slash if it doesn't already have one.
fn add_trailing_slash(url: &Url) -> Cow<'_, Url> {
    let path = url.path();
    if path.ends_with('/') {
        Cow::Borrowed(url)
    } else {
        let mut url = url.clone();
        url.set_path(&format!("{path}/"));
        Cow::Owned(url)
    }
}

async fn decode_zst_bytes_async<R: AsRef<[u8]> + Send + 'static>(
    bytes: R,
    url: Url,
) -> Result<Vec<u8>, GatewayError> {
    let decode = move || {
        let bytes_ref = bytes.as_ref();

        // Check for empty response which indicates a misconfigured server
        if bytes_ref.is_empty() {
            return Err(GatewayError::IoError(
                format!(
                    "failed to decode zstd shard from '{}': received empty response (0 bytes). \
                    This usually indicates a misconfigured server.",
                    url.redact()
                ),
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "empty response"),
            ));
        }

        match zstd::decode_all(bytes_ref) {
            Ok(decoded) => Ok(decoded),
            Err(err) => Err(GatewayError::IoError(
                format!(
                    "failed to decode zstd shard from '{}' ({} bytes received). \
                    The server may have returned invalid or truncated data.",
                    url.redact(),
                    bytes_ref.len()
                ),
                err,
            )),
        }
    };

    #[cfg(target_arch = "wasm32")]
    return decode();

    #[cfg(not(target_arch = "wasm32"))]
    simple_spawn_blocking::tokio::run_blocking_task(decode).await
}

async fn parse_records<R: AsRef<[u8]> + Send + 'static>(
    bytes: R,
    channel_base_url: ChannelUrl,
    base_url: Url,
) -> Result<PackageRecords, GatewayError> {
    let parse =
        move || {
            let shard = rmp_serde::from_slice::<Shard>(bytes.as_ref())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
                .map_err(FetchRepoDataError::IoError)?;

            // Chain v3 tar.bz2/conda packages into the main iteration
            let v3_tar_bz2 = shard.v3.tar_bz2.into_iter().map(|(id, rec)| {
                (
                    DistArchiveIdentifier::new(id, CondaArchiveType::TarBz2),
                    rec,
                )
            });
            let v3_conda =
                shard.v3.conda.into_iter().map(|(id, rec)| {
                    (DistArchiveIdentifier::new(id, CondaArchiveType::Conda), rec)
                });

            let packages = itertools::chain(shard.packages, shard.conda_packages)
                .chain(v3_tar_bz2)
                .chain(v3_conda)
                .filter(|(name, _record)| !shard.removed.contains(name));

            let channel_str = channel_base_url.url().clone().redact().to_string();
            let base_url_str = base_url.as_str();
            let mut records: Vec<Arc<RepoDataRecord>> = packages
                .map(|(file_name, package_record)| {
                    let file_name_str = file_name.to_file_name();
                    Arc::new(RepoDataRecord {
                        url: Url::parse(&format!("{base_url_str}{file_name_str}"))
                            .expect("filename is not a valid url"),
                        channel: Some(channel_str.clone()),
                        package_record,
                        identifier: file_name,
                    })
                })
                .collect();

            // Handle v3 whl packages separately (different URL resolution)
            for (
                id,
                WhlPackageRecord {
                    url,
                    package_record,
                },
            ) in shard.v3.whl
            {
                let dist_id = DistArchiveIdentifier::new(id, WheelArchiveType::Whl);
                let url = match url {
                    UrlOrPath::Path(path) => Url::parse(&format!("{base_url_str}{path}"))
                        .expect("path is not a valid url"),
                    UrlOrPath::Url(url) => url,
                };
                records.push(Arc::new(RepoDataRecord {
                    url,
                    channel: Some(channel_str.clone()),
                    package_record,
                    identifier: dist_id,
                }));
            }

            let (unique_base_deps, unique_extra_deps) =
                extract_unique_deps_split(records.iter().map(|r| &**r));
            Ok(PackageRecords {
                records,
                unique_base_deps,
                unique_extra_deps,
            })
        };

    #[cfg(target_arch = "wasm32")]
    return parse();

    #[cfg(not(target_arch = "wasm32"))]
    simple_spawn_blocking::tokio::run_blocking_task(parse).await
}

// Tests are only run on non-wasm targets since they use tokio and axum
#[cfg(test)]
mod tests {
    use crate::fetch::CacheAction;
    use crate::gateway::error::GatewayError;
    use crate::gateway::subdir::SubdirClient;
    use axum::{
        Router,
        body::Body,
        http::{Response, StatusCode},
        routing::get,
    };
    use rattler_conda_types::{Channel, RepodataRevisions, ShardedRepodata, ShardedSubdirInfo};
    #[cfg(feature = "experimental-virtual-package-plugins")]
    use rattler_conda_types::{PackageName, VirtualPackagePlugins};
    use rattler_digest::{Sha256, parse_digest_from_hex};
    use std::future::IntoFuture;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::oneshot;
    use url::Url;

    use super::{ShardCachePolicy, ShardedSubdir};

    /// A mock server that serves a sharded repodata index but returns
    /// configurable responses for shard requests.
    struct MockShardedServer {
        local_addr: SocketAddr,
        shard_requests: Arc<AtomicUsize>,
        _shutdown_sender: oneshot::Sender<()>,
    }

    impl MockShardedServer {
        async fn new(shard_response: MockShardResponse) -> Self {
            // Create a minimal sharded index with one package
            let mut shards = ahash::HashMap::default();
            // Use a known hash for the "test-package" shard (SHA256 of empty string)
            let shard_hash = parse_digest_from_hex::<Sha256>(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            )
            .unwrap();
            shards.insert("test-package".to_string(), shard_hash);

            let sharded_index = ShardedRepodata {
                info: ShardedSubdirInfo {
                    subdir: "linux-64".to_string(),
                    base_url: "./".to_string(),
                    shards_base_url: "./shards/".to_string(),
                    created_at: Some(jiff::Timestamp::now()),
                    repodata_revisions: RepodataRevisions::default(),
                    channel_relations: None,
                    #[cfg(feature = "experimental-virtual-package-plugins")]
                    virtual_package_plugins: mock_virtual_package_plugins(),
                },
                shards,
            };

            // Encode the index as msgpack and compress with zstd
            let index_bytes = rmp_serde::to_vec_named(&sharded_index).unwrap();
            let compressed_index = zstd::encode_all(index_bytes.as_slice(), 3).unwrap();

            let shard_requests = Arc::new(AtomicUsize::new(0));
            let app = Router::new()
                .route(
                    "/linux-64/repodata_shards.msgpack.zst",
                    get(move || async move {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("Content-Type", "application/octet-stream")
                            // Keep the cached copy fresh, so `UseCacheOnly`
                            // accepts it in the cold-shard tests.
                            .header("Cache-Control", "max-age=3600")
                            .body(Body::from(compressed_index.clone()))
                            .unwrap()
                    }),
                )
                .route(
                    "/linux-64/shards/{shard_file}",
                    get({
                        let shard_requests = Arc::clone(&shard_requests);
                        move || async move {
                            shard_requests.fetch_add(1, Ordering::SeqCst);
                            match shard_response {
                                MockShardResponse::Empty => Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Body::empty())
                                    .unwrap(),
                                MockShardResponse::Truncated => {
                                    // Return some bytes that look like zstd but are truncated
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Body::from(vec![0x28, 0xb5, 0x2f, 0xfd]))
                                        .unwrap()
                                }
                            }
                        }
                    }),
                );

            let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
            let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
            let local_addr = listener.local_addr().unwrap();

            let (tx, rx) = oneshot::channel();
            let server = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    rx.await.ok();
                })
                .into_future();

            tokio::spawn(server);

            Self {
                local_addr,
                shard_requests,
                _shutdown_sender: tx,
            }
        }

        fn url(&self) -> Url {
            Url::parse(&format!("http://localhost:{}", self.local_addr.port())).unwrap()
        }

        fn channel(&self) -> Channel {
            Channel::from_url(self.url())
        }

        /// How many shard downloads the server has answered so far. The index
        /// request is not counted.
        fn shard_request_count(&self) -> usize {
            self.shard_requests.load(Ordering::SeqCst)
        }
    }

    #[derive(Clone, Copy)]
    enum MockShardResponse {
        Empty,
        Truncated,
    }

    #[cfg(feature = "experimental-virtual-package-plugins")]
    fn mock_virtual_package_plugins() -> VirtualPackagePlugins {
        [(
            PackageName::new_unchecked("cuda-detect"),
            vec![
                PackageName::new_unchecked("__cuda"),
                PackageName::new_unchecked("__cuda_arch"),
            ],
        )]
        .into_iter()
        .collect()
    }

    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[tokio::test]
    async fn test_sharded_index_reports_virtual_package_plugins() {
        let server = MockShardedServer::new(MockShardResponse::Empty).await;
        let cache_dir = tempfile::tempdir().unwrap();

        let subdir = ShardedSubdir::new(
            server.channel(),
            "linux-64".to_string(),
            rattler_networking::LazyClient::default(),
            cache_dir.path().to_path_buf(),
            ShardCachePolicy {
                action: CacheAction::NoCache,
                missing_shards_are_empty: false,
            },
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            subdir.virtual_package_plugins(),
            &mock_virtual_package_plugins()
        );
    }

    #[tokio::test]
    async fn test_empty_shard_response_error() {
        let server = MockShardedServer::new(MockShardResponse::Empty).await;
        let channel = server.channel();
        let cache_dir = tempfile::tempdir().unwrap();

        let client = rattler_networking::LazyClient::default();

        let subdir = ShardedSubdir::new(
            channel,
            "linux-64".to_string(),
            client,
            cache_dir.path().to_path_buf(),
            ShardCachePolicy {
                action: CacheAction::NoCache,
                missing_shards_are_empty: false,
            },
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let package_name = "test-package".parse().unwrap();
        let result = subdir.fetch_package_records(&package_name, None).await;

        let err = result.expect_err("should fail with empty response");
        let err_string = err.to_string();

        // Redact the dynamic port number from the error message
        let err_string = regex::Regex::new(r"localhost:\d+")
            .unwrap()
            .replace_all(&err_string, "localhost:[PORT]")
            .to_string();

        insta::assert_snapshot!("empty_shard_response_error", err_string);
    }

    /// A mock server that returns a configurable HTTP status for the sharded
    /// repodata index URL. Used to exercise the fallback paths for servers
    /// that report the index as unavailable.
    async fn start_index_status_server(status: StatusCode) -> (SocketAddr, oneshot::Sender<()>) {
        let app = Router::new().route(
            "/linux-64/repodata_shards.msgpack.zst",
            get(move || async move {
                Response::builder()
                    .status(status)
                    .body(Body::empty())
                    .unwrap()
            }),
        );

        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let (tx, rx) = oneshot::channel();
        let server = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                rx.await.ok();
            })
            .into_future();

        tokio::spawn(server);

        (local_addr, tx)
    }

    async fn assert_index_status_triggers_subdir_not_found(status: StatusCode) {
        let (local_addr, _shutdown) = start_index_status_server(status).await;
        let channel = Channel::from_url(
            Url::parse(&format!("http://localhost:{}", local_addr.port())).unwrap(),
        );
        let cache_dir = tempfile::tempdir().unwrap();
        let client = rattler_networking::LazyClient::default();

        let err = ShardedSubdir::new(
            channel,
            "linux-64".to_string(),
            client,
            cache_dir.path().to_path_buf(),
            ShardCachePolicy {
                action: CacheAction::NoCache,
                missing_shards_are_empty: false,
            },
            None,
            None,
            None,
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected an error for status {status}"));

        assert!(
            matches!(err, GatewayError::SubdirNotFoundError(_)),
            "expected SubdirNotFoundError for status {status}, got {err}"
        );
    }

    #[tokio::test]
    async fn test_index_404_triggers_subdir_not_found() {
        assert_index_status_triggers_subdir_not_found(StatusCode::NOT_FOUND).await;
    }

    #[tokio::test]
    async fn test_index_501_triggers_subdir_not_found() {
        // JFrog Artifactory can produce 501s here
        assert_index_status_triggers_subdir_not_found(StatusCode::NOT_IMPLEMENTED).await;
    }

    #[tokio::test]
    async fn test_truncated_shard_response_error() {
        let server = MockShardedServer::new(MockShardResponse::Truncated).await;
        let channel = server.channel();
        let cache_dir = tempfile::tempdir().unwrap();

        let client = rattler_networking::LazyClient::default();

        let subdir = ShardedSubdir::new(
            channel,
            "linux-64".to_string(),
            client,
            cache_dir.path().to_path_buf(),
            ShardCachePolicy {
                action: CacheAction::NoCache,
                missing_shards_are_empty: false,
            },
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let package_name = "test-package".parse().unwrap();
        let result = subdir.fetch_package_records(&package_name, None).await;

        let err = result.expect_err("should fail with truncated response");
        let err_string = err.to_string();

        // Redact the dynamic port number from the error message
        let err_string = regex::Regex::new(r"localhost:\d+")
            .unwrap()
            .replace_all(&err_string, "localhost:[PORT]")
            .to_string();

        insta::assert_snapshot!("truncated_shard_response_error", err_string);
    }

    /// Warms the shard *index* cache without ever fetching a shard, then hands
    /// back a subdir that may only read from the cache. That is the state a
    /// cache-only query lands in when it reaches a package no earlier query
    /// walked.
    async fn cache_only_subdir_with_cold_shard(
        cache_dir: &Path,
        server: &MockShardedServer,
        cache_only_action: CacheAction,
        missing_shards_are_empty: bool,
    ) -> ShardedSubdir {
        let client = rattler_networking::LazyClient::default();

        ShardedSubdir::new(
            server.channel(),
            "linux-64".to_string(),
            client.clone(),
            cache_dir.to_path_buf(),
            ShardCachePolicy {
                action: CacheAction::CacheOrFetch,
                missing_shards_are_empty: false,
            },
            None,
            None,
            None,
        )
        .await
        .expect("the index is served, so it is cached now");

        ShardedSubdir::new(
            server.channel(),
            "linux-64".to_string(),
            client,
            cache_dir.to_path_buf(),
            ShardCachePolicy {
                action: cache_only_action,
                missing_shards_are_empty,
            },
            None,
            None,
            None,
        )
        .await
        .expect("the index comes from the cache")
    }

    /// The cache-only modes a cold shard behaves the same under.
    const CACHE_ONLY_ACTIONS: [CacheAction; 2] =
        [CacheAction::UseCacheOnly, CacheAction::ForceCacheOnly];

    #[tokio::test]
    async fn uncached_index_is_reported_as_such_in_cache_only_mode() {
        let server = MockShardedServer::new(MockShardResponse::Empty).await;
        let cache_dir = tempfile::tempdir().unwrap();

        let err = ShardedSubdir::new(
            server.channel(),
            "linux-64".to_string(),
            rattler_networking::LazyClient::default(),
            cache_dir.path().to_path_buf(),
            ShardCachePolicy {
                action: CacheAction::ForceCacheOnly,
                missing_shards_are_empty: true,
            },
            None,
            None,
            None,
        )
        .await
        .err()
        .expect("nothing is cached, and the index may not be fetched");

        assert!(
            matches!(err, GatewayError::ShardedIndexNotCached(_)),
            "expected ShardedIndexNotCached, got: {err}"
        );
    }

    #[tokio::test]
    async fn cold_shard_is_an_error_by_default() {
        for action in CACHE_ONLY_ACTIONS {
            let server = MockShardedServer::new(MockShardResponse::Empty).await;
            let cache_dir = tempfile::tempdir().unwrap();

            let subdir =
                cache_only_subdir_with_cold_shard(cache_dir.path(), &server, action, false).await;

            let err = subdir
                .fetch_package_records(&"test-package".parse().unwrap(), None)
                .await
                .expect_err("a cold shard fails a cache-only query");

            assert!(
                matches!(err, GatewayError::ShardNotCached(name) if name == "test-package"),
                "the error names the package whose shard is missing"
            );
            assert_eq!(
                server.shard_request_count(),
                0,
                "{action:?} may not download a shard"
            );
        }
    }

    #[tokio::test]
    async fn cold_shard_is_empty_when_opted_in() {
        for action in CACHE_ONLY_ACTIONS {
            let server = MockShardedServer::new(MockShardResponse::Empty).await;
            let cache_dir = tempfile::tempdir().unwrap();

            let subdir =
                cache_only_subdir_with_cold_shard(cache_dir.path(), &server, action, true).await;

            let records = subdir
                .fetch_package_records(&"test-package".parse().unwrap(), None)
                .await
                .expect("a cold shard is not an error when opted in");

            assert!(records.records.is_empty());
            assert_eq!(
                server.shard_request_count(),
                0,
                "{action:?} may not download a shard"
            );
        }
    }
}
