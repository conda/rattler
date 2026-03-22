use std::borrow::Cow;
use std::sync::Arc;

use cfg_if::cfg_if;
use rattler_conda_types::{
    package::{CondaArchiveType, DistArchiveIdentifier, WheelArchiveType},
    ChannelUrl, RepoDataRecord, Shard, UrlOrPath, WhlPackageRecord,
};
use rattler_redaction::Redact;
use url::Url;

use crate::{
    fetch::FetchRepoDataError,
    gateway::subdir::{extract_unique_deps, PackageRecords},
    GatewayError,
};

/// Returns `true` if the error is transient and the request should be retried.
/// This includes 429 (Too Many Requests), 5xx server errors, and connection
/// errors.
fn is_transient_error(err: &GatewayError) -> bool {
    match err {
        GatewayError::FetchRepoDataError(fetch_err) => match fetch_err {
            FetchRepoDataError::HttpError(err) => is_transient_reqwest_middleware_error(err),
            _ => false,
        },
        GatewayError::ReqwestError(err) => is_transient_reqwest_error(err),
        _ => false,
    }
}

fn is_transient_reqwest_middleware_error(err: &reqwest_middleware::Error) -> bool {
    match err {
        reqwest_middleware::Error::Reqwest(err) => is_transient_reqwest_error(err),
        _ => false,
    }
}

fn is_transient_reqwest_error(err: &reqwest::Error) -> bool {
    err.status()
        .is_some_and(|s| s == http::StatusCode::TOO_MANY_REQUESTS || s.is_server_error())
        || err.is_connect()
        || err.is_timeout()
}

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        mod wasm;
        pub use wasm::ShardedSubdir;
    } else {
        mod tokio;
        pub use tokio::ShardedSubdir;
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
            let v3_tar_bz2 = shard.experimental_v3.tar_bz2.into_iter().map(|(id, rec)| {
                (
                    DistArchiveIdentifier::new(id, CondaArchiveType::TarBz2),
                    rec,
                )
            });
            let v3_conda =
                shard.experimental_v3.conda.into_iter().map(|(id, rec)| {
                    (DistArchiveIdentifier::new(id, CondaArchiveType::Conda), rec)
                });

            let packages =
                itertools::chain(shard.packages.into_iter(), shard.conda_packages.into_iter())
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
            ) in shard.experimental_v3.whl
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

            let unique_deps = extract_unique_deps(records.iter().map(|r| &**r));
            Ok(PackageRecords {
                records,
                unique_deps,
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
    use crate::gateway::subdir::SubdirClient;
    use axum::{
        body::Body,
        extract::State,
        http::{Response, StatusCode},
        routing::get,
        Router,
    };
    use rattler_conda_types::{Channel, Shard, ShardedRepodata, ShardedSubdirInfo};
    use rattler_digest::{parse_digest_from_hex, Sha256};
    use std::future::IntoFuture;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use tokio::sync::oneshot;
    use url::Url;

    use super::ShardedSubdir;

    /// Shared state for the mock server to track request counts.
    #[derive(Clone)]
    struct MockState {
        shard_response: MockShardResponse,
        request_count: Arc<AtomicU32>,
    }

    /// A mock server that serves a sharded repodata index but returns
    /// configurable responses for shard requests.
    struct MockShardedServer {
        local_addr: SocketAddr,
        request_count: Arc<AtomicU32>,
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
                    created_at: Some(chrono::Utc::now()),
                },
                shards,
            };

            // Encode the index as msgpack and compress with zstd
            let index_bytes = rmp_serde::to_vec(&sharded_index).unwrap();
            let compressed_index = zstd::encode_all(index_bytes.as_slice(), 3).unwrap();

            let request_count = Arc::new(AtomicU32::new(0));
            let state = MockState {
                shard_response,
                request_count: request_count.clone(),
            };

            let app = Router::new()
                .route(
                    "/linux-64/repodata_shards.msgpack.zst",
                    get(move || async move {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("Content-Type", "application/octet-stream")
                            .body(Body::from(compressed_index.clone()))
                            .unwrap()
                    }),
                )
                .route(
                    "/linux-64/shards/{shard_file}",
                    get(|State(state): State<MockState>| async move {
                        let count = state.request_count.fetch_add(1, Ordering::SeqCst);
                        match state.shard_response {
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
                            MockShardResponse::TooManyRequests { fail_count } => {
                                if count < fail_count {
                                    Response::builder()
                                        .status(StatusCode::TOO_MANY_REQUESTS)
                                        .body(Body::empty())
                                        .unwrap()
                                } else {
                                    // Return a valid shard
                                    let shard = Shard {
                                        packages: Default::default(),
                                        conda_packages: Default::default(),
                                        removed: Default::default(),
                                        experimental_v3: Default::default(),
                                    };
                                    let shard_bytes = rmp_serde::to_vec(&shard).unwrap();
                                    let compressed =
                                        zstd::encode_all(shard_bytes.as_slice(), 3).unwrap();
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Body::from(compressed))
                                        .unwrap()
                                }
                            }
                        }
                    }),
                )
                .with_state(state);

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
                request_count,
                _shutdown_sender: tx,
            }
        }

        fn url(&self) -> Url {
            Url::parse(&format!("http://localhost:{}", self.local_addr.port())).unwrap()
        }

        fn channel(&self) -> Channel {
            Channel::from_url(self.url())
        }

        fn request_count(&self) -> u32 {
            self.request_count.load(Ordering::SeqCst)
        }
    }

    #[derive(Clone, Copy)]
    enum MockShardResponse {
        Empty,
        Truncated,
        /// Return 429 for the first `fail_count` requests, then succeed.
        TooManyRequests {
            fail_count: u32,
        },
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
            CacheAction::NoCache,
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
            CacheAction::NoCache,
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

    #[tokio::test]
    async fn test_429_retry_succeeds() {
        // Server returns 429 twice, then succeeds on the 3rd request
        let server =
            MockShardedServer::new(MockShardResponse::TooManyRequests { fail_count: 2 }).await;
        let channel = server.channel();
        let cache_dir = tempfile::tempdir().unwrap();

        let client = rattler_networking::LazyClient::default();

        let subdir = ShardedSubdir::new(
            channel,
            "linux-64".to_string(),
            client,
            cache_dir.path().to_path_buf(),
            CacheAction::NoCache,
            None,
            None,
        )
        .await
        .unwrap();

        let package_name = "test-package".parse().unwrap();
        let result = subdir.fetch_package_records(&package_name, None).await;

        // Should succeed after retries
        assert!(
            result.is_ok(),
            "expected success after retries, got: {result:?}"
        );
        // Should have made 3 requests (2 failures + 1 success)
        assert_eq!(server.request_count(), 3);
    }

    #[tokio::test]
    async fn test_429_retry_exhausted() {
        // Server always returns 429 (more failures than retries allow)
        let server =
            MockShardedServer::new(MockShardResponse::TooManyRequests { fail_count: 100 }).await;
        let channel = server.channel();
        let cache_dir = tempfile::tempdir().unwrap();

        let client = rattler_networking::LazyClient::default();

        let subdir = ShardedSubdir::new(
            channel,
            "linux-64".to_string(),
            client,
            cache_dir.path().to_path_buf(),
            CacheAction::NoCache,
            None,
            None,
        )
        .await
        .unwrap();

        let package_name = "test-package".parse().unwrap();
        let result = subdir.fetch_package_records(&package_name, None).await;

        // Should fail after exhausting retries
        let err = result.expect_err("should fail after exhausting retries");
        assert!(
            err.to_string().contains("429"),
            "error should mention 429: {err}"
        );
        // default_retry_policy retries 3 times, so 4 total requests (1 initial + 3 retries)
        assert_eq!(server.request_count(), 4);
    }
}
