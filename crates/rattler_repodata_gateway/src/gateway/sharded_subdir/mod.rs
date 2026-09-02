use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use cfg_if::cfg_if;
use http::StatusCode;
use rattler_conda_types::{
    ChannelUrl, PackageRecord, RepoDataRecord, Shard, UrlOrPath, WhlPackageRecord,
    package::{ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier, WheelArchiveType},
};
use rattler_redaction::Redact;
use url::Url;

use crate::{
    GatewayError,
    fetch::FetchRepoDataError,
    gateway::subdir::{PackageRecords, extract_unique_deps_split},
    sparse::PackageFormatSelection,
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

/// A raw, not-yet-converted-to-[`RepoDataRecord`] shard entry. Kept as an enum
/// (rather than eagerly building a `RepoDataRecord`) so that
/// [`dedup_by_preference`] can drop the losing variant of a (name, version,
/// build) group before the more expensive URL/record construction happens.
enum RawShardRecord {
    Package(PackageRecord),
    Whl(WhlPackageRecord),
}

/// Selects the shard entries relevant to `variant_consolidation`,
/// deduplicating preferred-format groups (e.g. `.conda` over `.tar.bz2`)
/// before any [`RepoDataRecord`] is built for a losing candidate.
fn select_shard_records(
    shard: Shard,
    variant_consolidation: PackageFormatSelection,
) -> impl Iterator<Item = (DistArchiveIdentifier, RawShardRecord)> {
    let Shard {
        packages,
        conda_packages,
        v3,
        removed,
    } = shard;

    let tar_bz2 = itertools::chain(
        packages,
        v3.tar_bz2.into_iter().map(|(id, rec)| {
            (
                DistArchiveIdentifier::new(id, CondaArchiveType::TarBz2),
                rec,
            )
        }),
    )
    .map(|(id, rec)| (id, RawShardRecord::Package(rec)));
    let conda = itertools::chain(
        conda_packages,
        v3.conda
            .into_iter()
            .map(|(id, rec)| (DistArchiveIdentifier::new(id, CondaArchiveType::Conda), rec)),
    )
    .map(|(id, rec)| (id, RawShardRecord::Package(rec)));
    let whl = v3.whl.into_iter().map(|(id, rec)| {
        (
            DistArchiveIdentifier::new(id, WheelArchiveType::Whl),
            RawShardRecord::Whl(rec),
        )
    });

    let selected: Vec<_> = match variant_consolidation {
        PackageFormatSelection::OnlyTarBz2 => tar_bz2.collect(),
        PackageFormatSelection::OnlyConda => conda.collect(),
        PackageFormatSelection::Both => tar_bz2.chain(conda).collect(),
        PackageFormatSelection::PreferConda => dedup_by_preference(tar_bz2.chain(conda)),
        PackageFormatSelection::PreferCondaWithWhl => {
            dedup_by_preference(tar_bz2.chain(conda).chain(whl))
        }
    };

    selected
        .into_iter()
        .filter(move |(id, _)| !removed.contains(id))
}

/// Keeps, for each unique (name, version, build) archive identifier, only the
/// most-preferred variant, per [`rattler_conda_types::package::DistArchiveType::cmp_preference`]
/// (`.conda` over `.tar.bz2` over `.whl`). The relative order of the
/// surviving entries is otherwise preserved.
fn dedup_by_preference(
    iter: impl Iterator<Item = (DistArchiveIdentifier, RawShardRecord)>,
) -> Vec<(DistArchiveIdentifier, RawShardRecord)> {
    let mut positions: std::collections::HashMap<ArchiveIdentifier, usize> =
        std::collections::HashMap::new();
    let mut out: Vec<(DistArchiveIdentifier, RawShardRecord)> = Vec::new();
    for (id, record) in iter {
        match positions.entry(id.identifier.clone()) {
            Entry::Occupied(entry) => {
                let idx = *entry.get();
                if id.archive_type.cmp_preference(out[idx].0.archive_type)
                    == std::cmp::Ordering::Greater
                {
                    out[idx] = (id, record);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(out.len());
                out.push((id, record));
            }
        }
    }
    out
}

/// Deserializes raw shard bytes into a `Shard`.
fn load_shard<R: AsRef<[u8]>>(bytes: R) -> Result<Shard, GatewayError> {
    rmp_serde::from_slice::<Shard>(bytes.as_ref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
        .map_err(FetchRepoDataError::IoError)
        .map_err(GatewayError::from)
}

/// Converts a `Shard` into `PackageRecords`, applying `variant_consolidation`
/// to select and deduplicate the package format variants exposed to the solver.
fn get_records(
    shard: Shard,
    channel_base_url: &ChannelUrl,
    base_url: &Url,
    variant_consolidation: PackageFormatSelection,
) -> PackageRecords {
    let channel_str = channel_base_url.url().clone().redact().to_string();
    let base_url_str = base_url.as_str();
    let records: Vec<Arc<RepoDataRecord>> = select_shard_records(shard, variant_consolidation)
        .map(|(file_name, raw_record)| match raw_record {
            RawShardRecord::Package(package_record) => {
                let file_name_str = file_name.to_file_name();
                Arc::new(RepoDataRecord {
                    url: Url::parse(&format!("{base_url_str}{file_name_str}"))
                        .expect("filename is not a valid url"),
                    channel: Some(channel_str.clone()),
                    package_record,
                    identifier: file_name,
                })
            }
            RawShardRecord::Whl(WhlPackageRecord {
                url,
                package_record,
            }) => {
                let url = match url {
                    UrlOrPath::Path(path) => Url::parse(&format!("{base_url_str}{path}"))
                        .expect("path is not a valid url"),
                    UrlOrPath::Url(url) => url,
                };
                Arc::new(RepoDataRecord {
                    url,
                    channel: Some(channel_str.clone()),
                    package_record,
                    identifier: file_name,
                })
            }
        })
        .collect();

    let (unique_base_deps, unique_extra_deps) =
        extract_unique_deps_split(records.iter().map(|r| &**r));
    PackageRecords {
        records,
        unique_base_deps,
        unique_extra_deps,
    }
}

// Tests are only run on non-wasm targets since they use tokio and axum
#[cfg(test)]
mod tests {
    use super::select_shard_records;
    use crate::gateway::error::GatewayError;
    use crate::gateway::subdir::SubdirClient;
    use crate::{fetch::CacheAction, sparse::PackageFormatSelection};
    use axum::{
        Router,
        body::Body,
        http::{Response, StatusCode},
        routing::get,
    };
    use rattler_conda_types::{
        Channel, PackageName, RepodataRevisions, Shard, ShardedRepodata, ShardedSubdirInfo,
        UrlOrPath, VersionWithSource, WhlPackageRecord,
        package::{ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier},
    };
    use rattler_digest::{Sha256, parse_digest_from_hex};
    use rstest::rstest;
    use std::future::IntoFuture;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::str::FromStr;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::oneshot;
    use url::Url;

    use super::{PackageRecord, ShardCachePolicy, ShardedSubdir};

    fn test_package_record(name: &str) -> PackageRecord {
        PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            VersionWithSource::from_str("1.0").unwrap(),
            "0".to_string(),
        )
    }

    fn archive_id(name: &str) -> ArchiveIdentifier {
        ArchiveIdentifier {
            name: name.to_string(),
            version: "1.0".to_string(),
            build_string: "0".to_string(),
        }
    }

    /// A package shipped as both `.tar.bz2` and `.conda`: `PreferConda` and
    /// `PreferCondaWithWhl` must keep only the `.conda` variant, without ever
    /// needing to build a `RepoDataRecord` for the discarded `.tar.bz2` one.
    #[rstest]
    #[case::prefer_conda(PackageFormatSelection::PreferConda)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl)]
    fn select_shard_records_prefers_conda_over_tar_bz2(#[case] selection: PackageFormatSelection) {
        let id = archive_id("foo");
        let mut shard = Shard::default();
        shard.packages.insert(
            DistArchiveIdentifier::new(id.clone(), CondaArchiveType::TarBz2),
            test_package_record("foo"),
        );
        shard.conda_packages.insert(
            DistArchiveIdentifier::new(id.clone(), CondaArchiveType::Conda),
            test_package_record("foo"),
        );

        let selected: Vec<_> = select_shard_records(shard, selection).collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0.archive_type, CondaArchiveType::Conda.into());
    }

    /// `OnlyTarBz2`/`OnlyConda` must only ever surface their own format, even
    /// when the other is present in the shard.
    #[rstest]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2, CondaArchiveType::TarBz2)]
    #[case::only_conda(PackageFormatSelection::OnlyConda, CondaArchiveType::Conda)]
    fn select_shard_records_only_selects_requested_format(
        #[case] selection: PackageFormatSelection,
        #[case] expected: CondaArchiveType,
    ) {
        let id = archive_id("foo");
        let mut shard = Shard::default();
        shard.packages.insert(
            DistArchiveIdentifier::new(id.clone(), CondaArchiveType::TarBz2),
            test_package_record("foo"),
        );
        shard.conda_packages.insert(
            DistArchiveIdentifier::new(id.clone(), CondaArchiveType::Conda),
            test_package_record("foo"),
        );

        let selected: Vec<_> = select_shard_records(shard, selection).collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0.archive_type, expected.into());
    }

    /// `.whl` is only ever considered for `PreferCondaWithWhl`; every other
    /// selection (including `Both`) must drop it.
    #[rstest]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2, false)]
    #[case::only_conda(PackageFormatSelection::OnlyConda, false)]
    #[case::both(PackageFormatSelection::Both, false)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda, false)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl, true)]
    fn select_shard_records_whl_only_for_prefer_conda_with_whl(
        #[case] selection: PackageFormatSelection,
        #[case] expect_whl: bool,
    ) {
        let id = archive_id("foo");
        let mut shard = Shard::default();
        shard.v3.whl.insert(
            id,
            WhlPackageRecord {
                url: UrlOrPath::Path("foo-1.0-0.whl".into()),
                package_record: test_package_record("foo"),
            },
        );

        let selected: Vec<_> = select_shard_records(shard, selection).collect();
        assert_eq!(selected.len(), usize::from(expect_whl));
    }

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
        let result = subdir
            .fetch_package_records(&package_name, None, PackageFormatSelection::default())
            .await;

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
        let result = subdir
            .fetch_package_records(&package_name, None, PackageFormatSelection::default())
            .await;

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
                .fetch_package_records(
                    &"test-package".parse().unwrap(),
                    None,
                    PackageFormatSelection::default(),
                )
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
                .fetch_package_records(
                    &"test-package".parse().unwrap(),
                    None,
                    PackageFormatSelection::default(),
                )
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
