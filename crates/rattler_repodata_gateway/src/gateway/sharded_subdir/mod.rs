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
        http::{Response, StatusCode},
        routing::get,
        Router,
    };
    use rattler_conda_types::{Channel, ShardedRepodata, ShardedSubdirInfo};
    use rattler_digest::{parse_digest_from_hex, Sha256};
    use std::future::IntoFuture;
    use std::net::SocketAddr;
    use tokio::sync::oneshot;
    use url::Url;

    use super::ShardedSubdir;

    /// A mock server that serves a sharded repodata index but returns
    /// configurable responses for shard requests.
    struct MockShardedServer {
        local_addr: SocketAddr,
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
                    get(move || async move {
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
                _shutdown_sender: tx,
            }
        }

        fn url(&self) -> Url {
            Url::parse(&format!("http://localhost:{}", self.local_addr.port())).unwrap()
        }

        fn channel(&self) -> Channel {
            Channel::from_url(self.url())
        }
    }

    #[derive(Clone, Copy)]
    enum MockShardResponse {
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
}
