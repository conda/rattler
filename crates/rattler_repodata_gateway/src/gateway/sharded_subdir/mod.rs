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
    use rattler_conda_types::{
        Channel, PackageName, Platform, RepodataRevisions, Shard, ShardedRepodata,
        ShardedSubdirInfo,
    };
    use rattler_digest::{Sha256, parse_digest_from_hex};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::{future::IntoFuture, str::FromStr};
    use tokio::sync::oneshot;
    use url::Url;

    use super::{ShardCachePolicy, ShardedSubdir};
    use crate::{Gateway, RepoDataQueryResult, ShardQuerySnapshot, SourceConfig};

    /// A mock server that serves a sharded repodata index but returns
    /// configurable responses for shard requests.
    struct MockShardedServer {
        local_addr: SocketAddr,
        sharded_index: Arc<Mutex<ShardedRepodata>>,
        index_requests: Arc<AtomicUsize>,
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
                },
                shards,
            };

            let sharded_index = Arc::new(Mutex::new(sharded_index));
            let valid_shard = zstd::encode_all(
                rmp_serde::to_vec_named(&Shard::default())
                    .unwrap()
                    .as_slice(),
                3,
            )
            .unwrap();

            let index_requests = Arc::new(AtomicUsize::new(0));
            let shard_requests = Arc::new(AtomicUsize::new(0));
            let app = Router::new()
                .route(
                    "/linux-64/repodata_shards.msgpack.zst",
                    get({
                        let sharded_index = Arc::clone(&sharded_index);
                        let index_requests = Arc::clone(&index_requests);
                        move || {
                            let sharded_index = Arc::clone(&sharded_index);
                            let index_requests = Arc::clone(&index_requests);
                            async move {
                                index_requests.fetch_add(1, Ordering::SeqCst);
                                let index_bytes =
                                    rmp_serde::to_vec_named(&*sharded_index.lock().unwrap())
                                        .unwrap();
                                let compressed_index =
                                    zstd::encode_all(index_bytes.as_slice(), 3).unwrap();
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .header("Content-Type", "application/octet-stream")
                                    .header("Cache-Control", "max-age=3600")
                                    .body(Body::from(compressed_index))
                                    .unwrap()
                            }
                        }
                    }),
                )
                .route(
                    "/linux-64/shards/{shard_file}",
                    get({
                        let shard_requests = Arc::clone(&shard_requests);
                        move || async move {
                            shard_requests.fetch_add(1, Ordering::SeqCst);
                            match shard_response {
                                MockShardResponse::Valid => Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Body::from(valid_shard.clone()))
                                    .unwrap(),
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
                sharded_index,
                index_requests,
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

        fn index_request_count(&self) -> usize {
            self.index_requests.load(Ordering::SeqCst)
        }

        fn refresh_created_at(&self) {
            self.sharded_index.lock().unwrap().info.created_at = Some(jiff::Timestamp::now());
        }

        fn replace_shard_hash(&self, hash: &str) {
            self.sharded_index.lock().unwrap().shards.insert(
                "test-package".to_string(),
                parse_digest_from_hex::<Sha256>(hash).unwrap(),
            );
        }

        /// How many shard downloads the server has answered so far. The index
        /// request is not counted.
        fn shard_request_count(&self) -> usize {
            self.shard_requests.load(Ordering::SeqCst)
        }
    }

    #[derive(Clone, Copy)]
    enum MockShardResponse {
        Valid,
        Empty,
        Truncated,
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

    #[tokio::test]
    async fn query_snapshot_ignores_volatile_index_metadata() {
        let server = MockShardedServer::new(MockShardResponse::Valid).await;
        let cache_dir = tempfile::tempdir().unwrap();
        let gateway = Gateway::builder()
            .with_cache_dir(cache_dir.path())
            .with_channel_config(crate::ChannelConfig {
                default: SourceConfig {
                    cache_action: CacheAction::NoCache,
                    ..SourceConfig::default()
                },
                per_channel: std::collections::HashMap::default(),
            })
            .finish();
        let package = PackageName::from_str("test-package").unwrap();

        let initial = gateway
            .query(
                vec![server.channel()],
                [Platform::Linux64],
                [package.clone()],
            )
            .recursive(true)
            .execute()
            .await
            .unwrap();
        let snapshot = initial
            .shard_query_snapshot()
            .cloned()
            .expect("exact-name sharded query has a replay snapshot");
        let serialized = serde_json::to_vec(&snapshot).unwrap();
        let round_tripped: ShardQuerySnapshot = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(round_tripped, snapshot);
        assert_eq!(server.shard_request_count(), 1);

        let mut forged = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(forged["version"], 1);
        assert_eq!(forged["query"]["channel_relations_mode"], "warn");
        forged["indexes"] = serde_json::Value::Array(Vec::new());
        let forged: ShardQuerySnapshot = serde_json::from_value(forged).unwrap();
        let replay = gateway
            .query(
                vec![server.channel()],
                [Platform::Linux64],
                [package.clone()],
            )
            .recursive(true)
            .execute_if_unchanged(&forged)
            .await
            .unwrap();
        assert!(matches!(replay, RepoDataQueryResult::Updated(_)));
        assert_eq!(server.shard_request_count(), 1);

        server.refresh_created_at();
        let replay = gateway
            .query(vec![server.channel()], [Platform::Linux64], [package])
            .recursive(true)
            .execute_if_unchanged(&snapshot)
            .await
            .unwrap();

        assert!(matches!(replay, RepoDataQueryResult::NotModified));
        assert_eq!(server.shard_request_count(), 1);
    }

    #[tokio::test]
    async fn query_snapshot_falls_back_when_a_shard_hash_changes() {
        let server = MockShardedServer::new(MockShardResponse::Valid).await;
        let cache_dir = tempfile::tempdir().unwrap();
        let gateway = Gateway::builder()
            .with_cache_dir(cache_dir.path())
            .with_channel_config(crate::ChannelConfig {
                default: SourceConfig {
                    cache_action: CacheAction::NoCache,
                    ..SourceConfig::default()
                },
                per_channel: std::collections::HashMap::default(),
            })
            .finish();
        let package = PackageName::from_str("test-package").unwrap();

        let initial = gateway
            .query(
                vec![server.channel()],
                [Platform::Linux64],
                [package.clone()],
            )
            .recursive(true)
            .execute()
            .await
            .unwrap();
        let snapshot = initial.shard_query_snapshot().cloned().unwrap();
        assert_eq!(server.index_request_count(), 1);
        server
            .replace_shard_hash("c3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");

        let replay = gateway
            .query(vec![server.channel()], [Platform::Linux64], [package])
            .recursive(true)
            .execute_if_unchanged(&snapshot)
            .await
            .unwrap();

        assert!(matches!(replay, RepoDataQueryResult::Updated(_)));
        assert_eq!(server.index_request_count(), 2);
        assert_eq!(server.shard_request_count(), 2);
    }

    /// The cache-only modes a cold shard behaves the same under.
    const CACHE_ONLY_ACTIONS: [CacheAction; 2] =
        [CacheAction::UseCacheOnly, CacheAction::ForceCacheOnly];

    /// A cache-only build with no index cached must report
    /// [`GatewayError::ShardedIndexNotCached`] and nothing else: that is the
    /// variant `SubdirBuilder` matches on to fall back to `repodata.json`,
    /// which may well be cached even when the sharded index is not.
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

    /// Without the opt-in, a cold shard fails a cache-only query with a
    /// distinct error: nothing is known about the package, which is not the
    /// same as the package having no records. Neither mode may touch the
    /// network to find out.
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

    /// With `missing_shards_are_empty` the same query reports the package as
    /// having no records, which lets a caller that restricts a solve to
    /// locally available packages fail on the restriction instead of on the
    /// cache. Neither mode may touch the network to find out.
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
