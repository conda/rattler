//! A streaming reverse-dependency query over a [`Gateway`](super::Gateway).
//!
//! Rather than materializing the repodata of the queried platforms (as a
//! wildcard [`RepoDataQuery`](super::RepoDataQuery) would), this query
//! scans the records of every package name one name at a time, keeps only
//! the matching records, and never inserts the scanned records into the
//! gateway's long-lived per-name cache. The memory held by the scan is
//! therefore bounded by the in-flight package scans instead of the complete
//! repodata of the queried platforms.
//!
//! What the scan does not bound is the result set, which for a common
//! package is the larger cost of the two. The query is therefore exposed as
//! a stream so a consumer can reduce each match and drop the record it came
//! from; [`WhoNeedsQuery::execute`] is the collecting convenience over it.
//!
//! The records of a channel source come from the full `repodata.json` of
//! each scanned subdir rather than from sharded repodata, whatever the
//! gateway's [`SourceConfig`](super::SourceConfig) prefers: reading every
//! package is one request against full repodata but one request per
//! package against shards. Only channels without usable full repodata are
//! read through their shards. See
//! [`GatewayInner::get_or_create_scan_subdir`](super::GatewayInner::get_or_create_scan_subdir)
//! for how those subdirs are kept apart from the ones ordinary queries
//! share.
//!
//! This module answers *where* the scanned records come from; what counts
//! as a reverse dependency is decided by [`crate::who_needs`].

use std::{future::IntoFuture, sync::Arc};

use futures::{
    StreamExt, TryStreamExt,
    stream::{self, FuturesUnordered},
};
use rattler_conda_types::{PackageName, Platform};

use super::{
    GatewayError, GatewayInner,
    boxed::{BoxFuture, BoxStream, box_future, box_stream},
    local_subdir::LocalSubdirClient,
    source::{CustomSourceClient, Source},
    subdir::{Subdir, SubdirData},
};
use crate::{
    Reporter,
    who_needs::{Dependent, WhoNeedsTarget, who_needs},
};

/// How many package names one batch task fetches and scans sequentially.
/// Batching amortizes the per-task overhead over many cheap per-name
/// fetches.
///
/// A batch collects its matches before yielding them, so together with
/// [`BATCH_CONCURRENCY`] this sets how many matches a stream buffers ahead
/// of its consumer. Scanning conda-forge for the dependents of `python`
/// (~490k matching records) holds ~190 MB at 100 names and 16 batches,
/// against ~500 MB at 500 names; larger batches do not scan faster.
const NAME_BATCH_SIZE: usize = 100;

/// A reverse-dependency query created through
/// [`Gateway::who_needs`](super::Gateway::who_needs).
///
/// When executed, every package of the queried sources and platforms is
/// scanned against the target. Records of scanned packages are dropped as
/// soon as the package is processed and are *not* inserted into the
/// gateway's per-name record cache (previously cached entries are still
/// reused), so a query over a large channel does not permanently grow the
/// gateway's memory footprint. Only the matching records are retained,
/// shared via `Arc` in the returned [`Dependent`]s.
///
/// The matches themselves can still be numerous enough to dominate memory —
/// half a million records depend on `python` in conda-forge. Use
/// [`stream`](Self::stream) to fold them as they arrive;
/// [`execute`](Self::execute) keeps every one of them.
///
/// Platforms are scanned one after another in the order they were passed in
/// (duplicate platforms are scanned once), and the name batches within a
/// subdir are scanned concurrently. See [`WhoNeedsTarget`] for the matching
/// semantics of its variants.
///
/// Unlike the other gateway queries, this one does not follow CEP-42
/// `channel_relations`: only the subdirs of the sources passed in are
/// scanned.
///
/// Channel sources are read through their full `repodata.json` (falling
/// back to sharded repodata only when a channel offers no usable full
/// repodata) regardless of the gateway's sharding preference; custom and
/// sparse sources are scanned as they are.
#[derive(Clone)]
pub struct WhoNeedsQuery {
    gateway: Arc<GatewayInner>,
    sources: Vec<Source>,
    platforms: Vec<Platform>,
    target: WhoNeedsTarget,
    reporter: Option<Arc<dyn Reporter>>,
}

impl WhoNeedsQuery {
    /// Constructs a new instance. This should not be called directly, use
    /// [`Gateway::who_needs`](super::Gateway::who_needs) instead.
    pub(super) fn new(
        gateway: Arc<GatewayInner>,
        sources: Vec<Source>,
        platforms: Vec<Platform>,
        target: WhoNeedsTarget,
    ) -> Self {
        Self {
            gateway,
            sources,
            platforms,
            target,
            reporter: None,
        }
    }

    /// Sets the reporter to use for this query.
    pub fn with_reporter(self, reporter: impl Reporter + 'static) -> Self {
        Self {
            reporter: Some(Arc::new(reporter)),
            ..self
        }
    }

    /// Execute the query and return all reverse dependencies of the target,
    /// in the order described by [`Self::stream`].
    ///
    /// This collects the entire result set into memory. A channel-wide
    /// query can match a lot of records — every record depending on
    /// `python` in conda-forge is roughly half a million, about a gigabyte
    /// once each is retained — so prefer [`Self::stream`] when the results
    /// can be folded into something smaller as they arrive.
    pub async fn execute(self) -> Result<Vec<Dependent>, GatewayError> {
        self.stream().try_collect().await
    }

    /// Execute the query as a stream of reverse dependencies.
    ///
    /// Records are fetched, scanned and dropped as the stream is polled, so
    /// a consumer that aggregates each [`Dependent`] and drops it keeps only
    /// its own aggregate in memory rather than the whole result set. Nothing
    /// is fetched until the stream is first polled, and dropping the stream
    /// stops the scan.
    ///
    /// Items arrive in the same order as [`Self::execute`] returns them:
    /// grouped by the platform order passed to
    /// [`Gateway::who_needs`](super::Gateway::who_needs), then by source
    /// order, with the order within a subdir unspecified.
    ///
    /// The stream is boxed so it can be polled without pinning it first.
    ///
    /// ```no_run
    /// # use futures::TryStreamExt;
    /// # use rattler_conda_types::{Channel, PackageName, Platform};
    /// # use rattler_repodata_gateway::Gateway;
    /// # async fn example(gateway: Gateway, channel: Channel, name: PackageName) -> anyhow::Result<()> {
    /// let mut stream = gateway
    ///     .who_needs(vec![channel], vec![Platform::Linux64], name)
    ///     .stream();
    ///
    /// // Count the dependents while holding only one record at a time.
    /// let mut count = 0;
    /// while let Some(dependent) = stream.try_next().await? {
    ///     count += 1;
    ///     drop(dependent);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn stream(self) -> BoxStream<Result<Dependent, GatewayError>> {
        // Deduplicate platforms while keeping the input order, so the
        // result order stays deterministic and no subdir is scanned twice.
        let mut seen_platforms = std::collections::HashSet::new();
        let platforms: Vec<Platform> = self
            .platforms
            .iter()
            .copied()
            .filter(|p| seen_platforms.insert(*p))
            .collect();

        // Platforms are scanned in sequence rather than concurrently. That
        // is what makes the output order match the input platform order
        // without buffering a whole platform's results to re-sort them, and
        // it bounds the number of in-flight scans; the concurrency that
        // matters is between the name batches of a subdir.
        box_stream(stream::iter(platforms).flat_map(move |platform| {
            scan_platform(
                self.gateway.clone(),
                self.sources.clone(),
                platform,
                self.target.clone(),
                self.reporter.clone(),
            )
        }))
    }
}

impl IntoFuture for WhoNeedsQuery {
    type Output = Result<Vec<Dependent>, GatewayError>;
    type IntoFuture = BoxFuture<Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        box_future(self.execute())
    }
}

/// Runs a scan on its own tokio task so scans run on the runtime's worker
/// threads instead of interleaving on the task driving the query.
#[cfg(not(target_arch = "wasm32"))]
async fn spawn_scan<T: Send + 'static>(
    scan: impl Future<Output = Result<T, GatewayError>> + Send + 'static,
) -> Result<T, GatewayError> {
    tokio::spawn(scan).await.expect("the scan task panicked")
}

/// On wasm there are no threads to parallelize over; run the scan in place.
#[cfg(target_arch = "wasm32")]
async fn spawn_scan<T>(
    scan: impl Future<Output = Result<T, GatewayError>>,
) -> Result<T, GatewayError> {
    scan.await
}

/// A resolved subdir tagged with the index of the source it came from.
type IndexedSubdir = (usize, Arc<Subdir>);

/// Streams the dependents of `target` found on `platform`, across the
/// subdirs of every source, in the caller's source order.
fn scan_platform(
    gateway: Arc<GatewayInner>,
    sources: Vec<Source>,
    platform: Platform,
    target: WhoNeedsTarget,
    reporter: Option<Arc<dyn Reporter>>,
) -> BoxStream<Result<Dependent, GatewayError>> {
    // The subdirs of a platform are resolved when the platform is first
    // polled, not when the stream is built, so a consumer that stops early
    // never pays to resolve the platforms it did not reach.
    box_stream(
        stream::once(resolve_subdirs(
            gateway,
            sources,
            platform,
            reporter.clone(),
        ))
        .map_ok(move |subdirs| {
            let target = target.clone();
            let reporter = reporter.clone();
            stream::iter(subdirs)
                .map(move |(_, subdir)| scan_subdir(subdir, target.clone(), reporter.clone()))
                // Subdirs are scanned one after another so the result order
                // follows the caller's source order.
                .flatten()
        })
        .try_flatten(),
    )
}

/// Resolves the subdirs of every source for `platform`, in the caller's
/// source order.
async fn resolve_subdirs(
    gateway: Arc<GatewayInner>,
    sources: Vec<Source>,
    platform: Platform,
    reporter: Option<Arc<dyn Reporter>>,
) -> Result<Vec<IndexedSubdir>, GatewayError> {
    // Kick off the subdir fetch of every channel source; custom and sparse
    // sources resolve immediately. Each subdir is tagged with the index of
    // the source it came from so the scan order follows the caller's
    // source order regardless of which fetch finishes first.
    let mut pending: FuturesUnordered<BoxFuture<Result<IndexedSubdir, GatewayError>>> =
        FuturesUnordered::new();
    let mut subdirs: Vec<IndexedSubdir> = Vec::new();

    for (source_index, source) in sources.into_iter().enumerate() {
        match source {
            Source::Channel(channel) => {
                let gateway = gateway.clone();
                let reporter = reporter.clone();
                pending.push(box_future(async move {
                    // A scan reads every package of the subdir, which is one
                    // request against full repodata but one per package
                    // against shards, so the subdir is built to prefer full
                    // repodata whatever the gateway's sharding preference.
                    let subdir = gateway
                        .get_or_create_scan_subdir(&channel, platform, reporter)
                        .await?;
                    Ok((source_index, subdir))
                }));
            }
            Source::Custom(custom_source) => {
                let client = CustomSourceClient::new(custom_source, platform);
                subdirs.push((
                    source_index,
                    Arc::new(Subdir::Found(SubdirData::from_client(client))),
                ));
            }
            Source::SparseRepoData(sparse_list) => {
                let subdir = match sparse_list
                    .iter()
                    .find(|sparse| platform.as_str() == sparse.subdir())
                {
                    Some(sparse) => Arc::new(Subdir::Found(SubdirData::from_client(
                        LocalSubdirClient::new(sparse.clone()),
                    ))),
                    None => Arc::new(Subdir::NotFound),
                };
                subdirs.push((source_index, subdir));
            }
        }
    }

    while let Some(result) = pending.next().await {
        subdirs.push(result?);
    }
    subdirs.sort_by_key(|(source_index, _)| *source_index);
    Ok(subdirs)
}

/// How many name batches of one subdir are scanned concurrently.
///
/// This is what bounds the memory of a stream: at most this many batches'
/// worth of matches are buffered before the consumer sees them, so a
/// consumer that folds results as they arrive never holds the whole result
/// set. Concurrency is per subdir and subdirs are scanned in sequence, so
/// the in-flight batch count of a whole query stays bounded by this.
const BATCH_CONCURRENCY: usize = 16;

/// Streams the dependents found in `subdir`, scanning it in batches of
/// [`NAME_BATCH_SIZE`] names.
///
/// Each batch runs as its own task (see [`spawn_scan`]) that fetches one
/// package at a time, keeps the matches, and drops the scanned records
/// before fetching the next package. The scanned records are never inserted
/// into the subdir's per-name record cache. Batches are polled in order and
/// at most [`BATCH_CONCURRENCY`] run at once, so matches reach the consumer
/// while the rest of the subdir is still being scanned.
fn scan_subdir(
    subdir: Arc<Subdir>,
    target: WhoNeedsTarget,
    reporter: Option<Arc<dyn Reporter>>,
) -> BoxStream<Result<Dependent, GatewayError>> {
    let names: Vec<PackageName> = match subdir.as_ref() {
        Subdir::Found(subdir_data) => subdir_data
            .package_names()
            .into_iter()
            .filter_map(|name| PackageName::try_from(name).ok())
            .collect(),
        Subdir::NotFound => Vec::new(),
    };
    let batches: Vec<Vec<PackageName>> = names
        .chunks(NAME_BATCH_SIZE)
        .map(<[PackageName]>::to_vec)
        .collect();

    box_stream(
        stream::iter(batches)
            .map(move |batch| {
                let subdir = subdir.clone();
                let target = target.clone();
                let reporter = reporter.clone();
                spawn_scan(async move {
                    let Subdir::Found(subdir_data) = subdir.as_ref() else {
                        return Ok(Vec::new());
                    };
                    let mut matches = Vec::new();
                    for name in batch {
                        let records = subdir_data
                            .fetch_package_records_uncached(&name, reporter.as_deref())
                            .await?;
                        matches.extend(who_needs(&records, &target));
                        // The scanned records are dropped here; only the
                        // matches survive.
                    }
                    Ok(matches)
                })
            })
            // `buffered` keeps the batch order while running up to
            // `BATCH_CONCURRENCY` of them at once, which is what caps how
            // many matches are in memory ahead of the consumer.
            .buffered(BATCH_CONCURRENCY)
            // Flatten each batch's `Vec<Dependent>` into individual items so
            // a consumer can drop each dependent as it goes.
            .map_ok(|matches| stream::iter(matches.into_iter().map(Ok)))
            .try_flatten(),
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::{path::Path, str::FromStr};

    use rattler_conda_types::{Channel, PackageName, Platform};

    use super::super::Gateway;
    use crate::who_needs::{DependencyKind, Dependent, WhoNeedsTarget};

    fn local_channel(name: &str) -> Channel {
        Channel::try_from_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../test-data/channels/{name}")),
        )
        .unwrap()
    }

    async fn local_conda_forge() -> Channel {
        tokio::try_join!(
            tools::fetch_test_conda_forge_repodata_async("noarch"),
            tools::fetch_test_conda_forge_repodata_async("linux-64")
        )
        .unwrap();
        local_channel("conda-forge")
    }

    /// Renders the dependents as one sorted `kind | package | dependency`
    /// line each, for snapshot comparison. Sorting makes the rendering
    /// independent of the unspecified within-platform result order.
    fn render(dependents: &[Dependent]) -> String {
        let mut lines: Vec<String> = dependents
            .iter()
            .map(|dependent| {
                let record = &dependent.record.package_record;
                let kind = match &dependent.kind {
                    DependencyKind::Depends => "depends".to_string(),
                    DependencyKind::Constrains => "constrains".to_string(),
                    DependencyKind::ExtraDepends(extra) => format!("extra_depends[{extra}]"),
                    DependencyKind::RunExport(kind) => format!("run_export[{kind:?}]"),
                };
                format!(
                    "{kind} | {}-{}-{} | {}",
                    record.name.as_normalized(),
                    record.version,
                    record.build,
                    dependent.dependency
                )
            })
            .collect();
        lines.sort();
        lines.join("\n")
    }

    /// The dependents of `target` in the `dummy` channel, rendered.
    async fn who_needs_dummy(channel: &str, platform: Platform, target: WhoNeedsTarget) -> String {
        let dependents = Gateway::new()
            .who_needs(vec![local_channel(channel)], vec![platform], target)
            .execute()
            .await
            .unwrap();
        render(&dependents)
    }

    /// A name target reports every dependency naming the package,
    /// regardless of its version constraints, across `depends` and
    /// `constrains`.
    #[tokio::test]
    async fn test_who_needs_name_target() {
        insta::assert_snapshot!(
            who_needs_dummy(
                "dummy",
                Platform::Linux64,
                PackageName::from_str("bors").unwrap().into(),
            )
            .await,
            @r###"
        constrains | foo-3.0.2-py36h1af98f8_3 | bors <2.0
        depends | foobar-2.0-bla_1 | bors <2.0
        depends | foobar-2.1-bla_1 | bors <2.0
        "###
        );
    }

    /// A concrete record target only reports dependents whose match spec
    /// matches it: the `bors <2.0` edges disappear for `bors 2.1`.
    #[tokio::test]
    async fn test_who_needs_record_target() {
        let bors_1_1 = record(&local_channel("dummy"), Platform::Linux64, "bors", "1.1").await;
        insta::assert_snapshot!(
            who_needs_dummy("dummy", Platform::Linux64, bors_1_1.into()).await,
            @r###"
        constrains | foo-3.0.2-py36h1af98f8_3 | bors <2.0
        depends | foobar-2.0-bla_1 | bors <2.0
        depends | foobar-2.1-bla_1 | bors <2.0
        "###
        );

        let bors_2_1 = record(&local_channel("dummy"), Platform::Linux64, "bors", "2.1").await;
        insta::assert_snapshot!(
            who_needs_dummy("dummy", Platform::Linux64, bors_2_1.into()).await,
            @""
        );
    }

    /// A virtual package target matches the `__unix` / `__cuda` edges.
    #[tokio::test]
    async fn test_who_needs_virtual_package_target() {
        let cuda = rattler_conda_types::GenericVirtualPackage {
            name: PackageName::from_str("__cuda").unwrap(),
            version: rattler_conda_types::Version::from_str("12.5").unwrap(),
            build_string: "0".to_string(),
        };
        insta::assert_snapshot!(
            who_needs_dummy("dummy", Platform::Linux64, cuda.into()).await,
            @"constrains | cuda-version-12.5-hd4f0392_3 | __cuda >=12.1"
        );
    }

    /// Dependencies declared under an optional feature are reported with
    /// the name of the extra, and a concrete target still has to satisfy
    /// the extra's constraints - `bar 1` matches `extra1`'s `bar <2` but
    /// not `extra2`'s `bar >=2`.
    #[tokio::test]
    async fn test_who_needs_extra_depends() {
        let channel = "dummy-optional-dependencies";
        insta::assert_snapshot!(
            who_needs_dummy(
                channel,
                Platform::NoArch,
                PackageName::from_str("bar").unwrap().into(),
            )
            .await,
            @r###"
        extra_depends[extra1] | conflicting-extras-1-xxx | bar <2
        extra_depends[extra2] | conflicting-extras-1-xxx | bar >=2
        extra_depends[with-bar] | foo-1-xxx | bar <2
        "###
        );

        let bar_1 = record(&local_channel(channel), Platform::NoArch, "bar", "1").await;
        insta::assert_snapshot!(
            who_needs_dummy(channel, Platform::NoArch, bar_1.into()).await,
            @r###"
        extra_depends[extra1] | conflicting-extras-1-xxx | bar <2
        extra_depends[with-bar] | foo-1-xxx | bar <2
        "###
        );
    }

    /// Fetches the record of `name` at `version` from `channel`, to use as
    /// a concrete [`WhoNeedsTarget`].
    async fn record(
        channel: &Channel,
        platform: Platform,
        name: &str,
        version: &str,
    ) -> rattler_conda_types::PackageRecord {
        Gateway::new()
            .query(
                vec![channel.clone()],
                vec![platform],
                vec![PackageName::from_str(name).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .unwrap()
            .repodata
            .iter()
            .flat_map(|repodata| repodata.iter())
            .find(|record| record.package_record.version.to_string() == version)
            .unwrap_or_else(|| panic!("{name} {version} is missing from the test channel"))
            .package_record
            .clone()
    }

    #[tokio::test]
    async fn test_who_needs_platform_order_and_duplicates() {
        let channel = local_conda_forge().await;
        let gateway = Gateway::new();
        let target = WhoNeedsTarget::from(PackageName::from_str("python_abi").unwrap());

        let dependents = gateway
            .who_needs(
                vec![channel.clone()],
                // The duplicate platform must be scanned only once.
                vec![Platform::Linux64, Platform::NoArch, Platform::Linux64],
                target.clone(),
            )
            .execute()
            .await
            .unwrap();

        // Results are grouped by input platform order: all linux-64
        // dependents come before all noarch dependents.
        let subdirs: Vec<&str> = dependents
            .iter()
            .map(|dependent| dependent.record.package_record.subdir.as_str())
            .collect();
        let first_noarch = subdirs.iter().position(|subdir| *subdir == "noarch");
        if let Some(first_noarch) = first_noarch {
            assert!(
                subdirs[first_noarch..]
                    .iter()
                    .all(|subdir| *subdir == "noarch"),
                "linux-64 dependents interleaved with noarch dependents"
            );
        }

        // The duplicate platform did not duplicate results.
        let deduplicated = gateway
            .who_needs(
                vec![channel],
                vec![Platform::Linux64, Platform::NoArch],
                target,
            )
            .execute()
            .await
            .unwrap();
        assert_eq!(dependents.len(), deduplicated.len());
    }

    #[tokio::test]
    async fn test_who_needs_does_not_populate_record_cache() {
        let channel = local_conda_forge().await;
        let gateway = Gateway::new();

        // Prime the cache with a single package through an ordinary query.
        let python = PackageName::from_str("python").unwrap();
        gateway
            .query(
                vec![channel.clone()],
                vec![Platform::Linux64],
                vec![python.clone()],
            )
            .recursive(false)
            .execute()
            .await
            .unwrap();

        let linux_subdir = gateway
            .inner
            .get_or_create_subdir(&channel, Platform::Linux64, None)
            .await
            .unwrap();
        let super::Subdir::Found(linux_data) = linux_subdir.as_ref() else {
            panic!("expected the linux-64 subdir to exist");
        };
        assert_eq!(linux_data.cached_package_count(), 1);

        // The reverse dependency scan must reuse the cached entry without
        // inserting the thousands of other scanned packages.
        let dependents = gateway
            .who_needs(
                vec![channel.clone()],
                vec![Platform::Linux64, Platform::NoArch],
                PackageName::from_str("python_abi").unwrap(),
            )
            .execute()
            .await
            .unwrap();
        assert!(!dependents.is_empty());

        assert_eq!(linux_data.cached_package_count(), 1);
        let noarch_subdir = gateway
            .inner
            .get_or_create_subdir(&channel, Platform::NoArch, None)
            .await
            .unwrap();
        let super::Subdir::Found(noarch_data) = noarch_subdir.as_ref() else {
            panic!("expected the noarch subdir to exist");
        };
        assert_eq!(noarch_data.cached_package_count(), 0);

        // The previously cached records are still usable afterwards.
        let records = gateway
            .query(vec![channel], vec![Platform::Linux64], vec![python])
            .recursive(false)
            .execute()
            .await
            .unwrap();
        assert!(records.repodata.iter().any(|repodata| !repodata.is_empty()));
    }

    #[tokio::test]
    async fn test_who_needs_propagates_errors() {
        let gateway = Gateway::new();
        let channel =
            Channel::from_url(url::Url::parse("file:///definitely/does/not/exist").unwrap());
        // A missing subdir is treated as empty for every platform except
        // noarch (same as `RepoDataQuery`), so query noarch to observe the
        // fetch error of a user-supplied channel propagating.
        let result = gateway
            .who_needs(
                vec![channel],
                vec![Platform::NoArch],
                PackageName::from_str("python").unwrap(),
            )
            .execute()
            .await;
        assert!(result.is_err());
    }

    /// Tests of how `who_needs` reads remote channels: it prefers the full
    /// `repodata.json` of a subdir over sharded repodata regardless of the
    /// gateway's `SourceConfig`, keeps the subdirs it builds apart from the
    /// ones ordinary queries share, and falls back to shards when a channel
    /// offers no usable full repodata.
    mod remote_channel {
        use std::{
            path::Path,
            str::FromStr,
            sync::{Arc, Mutex},
        };

        use assert_matches::assert_matches;
        use rattler_conda_types::{
            Channel, ChannelInfo, PackageName, PackageRecord, Platform, RepoData, RepoDataRecord,
            RepodataRevisions, Shard, ShardedRepodata, ShardedSubdirInfo, V3Packages,
            VersionWithSource, package::DistArchiveIdentifier,
        };
        use rattler_digest::{Sha256, compute_bytes_digest};
        use url::Url;

        use super::render;
        use crate::{
            ChannelConfig, DownloadReporter, GatewayError, Reporter, SourceConfig,
            fetch::{CacheAction, FetchRepoDataError},
            gateway::{CacheClearMode, Gateway, RepoDataSource, Source, SubdirSelection},
            utils::simple_channel_server::SimpleChannelServer,
        };

        const SUBDIR: &str = "linux-64";

        /// The dependents of `bors` in [`records`], as [`render`] shows them.
        const BORS_DEPENDENTS: &str =
            "constrains | bar-1.0-0 | bors <2\ndepends | foo-1.0-0 | bors >=1";

        /// A `linux-64` record for the test channel.
        fn record(name: &str, depends: &[&str], constrains: &[&str]) -> PackageRecord {
            let mut record = PackageRecord::new(
                PackageName::from_str(name).unwrap(),
                VersionWithSource::from_str("1.0").unwrap(),
                "0".to_string(),
            );
            record.subdir = SUBDIR.to_string();
            record.depends = depends.iter().map(ToString::to_string).collect();
            record.constrains = constrains.iter().map(ToString::to_string).collect();
            record
        }

        /// The records of the test channel: `foo` depends on `bors`, `bar`
        /// constrains it, and `baz` and `bors` itself do not reference it.
        fn records() -> Vec<PackageRecord> {
            vec![
                record("bors", &[], &[]),
                record("foo", &["bors >=1"], &[]),
                record("bar", &[], &["bors <2"]),
                record("baz", &["foo"], &[]),
            ]
        }

        fn identifier(record: &PackageRecord) -> DistArchiveIdentifier {
            format!(
                "{}-{}-{}.conda",
                record.name.as_normalized(),
                record.version,
                record.build
            )
            .parse()
            .unwrap()
        }

        /// The full `repodata.json` of the test channel's subdir.
        fn full_repodata() -> String {
            let repodata = RepoData {
                info: Some(ChannelInfo {
                    subdir: Some(SUBDIR.to_string()),
                    base_url: None,
                    repodata_revisions: RepodataRevisions::default(),
                    channel_relations: None,
                }),
                packages: std::iter::empty().collect(),
                conda_packages: records()
                    .into_iter()
                    .map(|record| (identifier(&record), record))
                    .collect(),
                v3: V3Packages::default(),
                removed: ahash::HashSet::default(),
                version: Some(2),
            };
            serde_json::to_string(&repodata).unwrap()
        }

        /// Which repodata artifacts a test channel serves for its subdir.
        #[derive(Clone, Copy)]
        struct Artifacts {
            /// The full `repodata.json`.
            full: bool,
            /// A sharded index with one shard per package.
            sharded: bool,
        }

        const FULL_AND_SHARDED: Artifacts = Artifacts {
            full: true,
            sharded: true,
        };
        const FULL_ONLY: Artifacts = Artifacts {
            full: true,
            sharded: false,
        };
        const SHARDED_ONLY: Artifacts = Artifacts {
            full: false,
            sharded: true,
        };

        /// Writes the `linux-64` subdir of the test channel to `root` with
        /// the requested artifacts, so a [`SimpleChannelServer`] can serve it.
        fn write_channel(root: &Path, artifacts: Artifacts) {
            let subdir = root.join(SUBDIR);
            std::fs::create_dir_all(subdir.join("shards")).unwrap();

            if artifacts.full {
                std::fs::write(subdir.join("repodata.json"), full_repodata()).unwrap();
            }

            if artifacts.sharded {
                let mut shards = ahash::HashMap::default();
                for record in records() {
                    let mut shard = Shard::default();
                    let name = record.name.as_normalized().to_string();
                    shard.conda_packages.insert(identifier(&record), record);
                    let bytes = rmp_serde::to_vec_named(&shard).unwrap();
                    let hash = compute_bytes_digest::<Sha256>(&bytes);
                    std::fs::write(
                        subdir.join(format!("shards/{}.msgpack.zst", hex::encode(hash))),
                        zstd::encode_all(bytes.as_slice(), 3).unwrap(),
                    )
                    .unwrap();
                    shards.insert(name, hash);
                }
                let index = ShardedRepodata {
                    info: ShardedSubdirInfo {
                        subdir: SUBDIR.to_string(),
                        base_url: "./".to_string(),
                        shards_base_url: "./shards/".to_string(),
                        created_at: None,
                        repodata_revisions: RepodataRevisions::default(),
                        channel_relations: None,
                    },
                    shards,
                };
                let bytes = rmp_serde::to_vec_named(&index).unwrap();
                std::fs::write(
                    subdir.join("repodata_shards.msgpack.zst"),
                    zstd::encode_all(bytes.as_slice(), 3).unwrap(),
                )
                .unwrap();
            }
        }

        /// A served copy of the test channel. The directory must outlive the
        /// server, so both are kept together.
        struct TestChannel {
            server: SimpleChannelServer,
            _dir: tempfile::TempDir,
        }

        impl TestChannel {
            async fn serve(artifacts: Artifacts) -> Self {
                let dir = tempfile::tempdir().unwrap();
                write_channel(dir.path(), artifacts);
                Self {
                    server: SimpleChannelServer::new(dir.path()).await,
                    _dir: dir,
                }
            }

            fn channel(&self) -> Channel {
                self.server.channel()
            }
        }

        /// Counts the repodata artifacts the gateway downloaded, by kind.
        #[derive(Default)]
        struct Downloads {
            urls: Mutex<Vec<Url>>,
        }

        impl Downloads {
            fn count(&self, kind: fn(&Url) -> bool) -> usize {
                self.urls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|url| kind(url))
                    .count()
            }

            /// Downloads of the full `repodata.json` or one of its
            /// compressed variants.
            fn full(&self) -> usize {
                self.count(|url| {
                    url.path()
                        .rsplit('/')
                        .next()
                        .is_some_and(|file| file.starts_with("repodata.json"))
                })
            }

            /// Downloads of the sharded repodata index.
            fn indexes(&self) -> usize {
                self.count(|url| url.path().ends_with("repodata_shards.msgpack.zst"))
            }

            /// Downloads of individual shards.
            fn shards(&self) -> usize {
                self.count(|url| url.path().contains("/shards/"))
            }

            fn clear(&self) {
                self.urls.lock().unwrap().clear();
            }
        }

        impl DownloadReporter for Arc<Downloads> {
            fn on_download_complete(&self, url: &Url, _index: usize) {
                self.urls.lock().unwrap().push(url.clone());
            }
        }

        impl Reporter for Arc<Downloads> {
            fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
                Some(self)
            }
        }

        /// A gateway with the given cache behavior and sharding preference,
        /// caching on disk under `cache_dir`.
        fn gateway(cache_action: CacheAction, sharded_enabled: bool, cache_dir: &Path) -> Gateway {
            Gateway::builder()
                .with_cache_dir(cache_dir)
                .with_channel_config(ChannelConfig {
                    default: SourceConfig {
                        sharded_enabled,
                        cache_action,
                        ..SourceConfig::default()
                    },
                    ..ChannelConfig::default()
                })
                .finish()
        }

        /// A gateway with the default (sharding enabled) configuration that
        /// never reads the on-disk cache, so every subdir it builds shows up
        /// as downloads. Only the in-memory subdir caches dedupe requests.
        fn uncached_gateway(cache_dir: &Path) -> Gateway {
            gateway(CacheAction::NoCache, true, cache_dir)
        }

        /// Runs `who_needs` for `bors` on `channel` and renders the result.
        async fn who_needs_bors(
            gateway: &Gateway,
            source: impl Into<Source>,
            reporter: &Arc<Downloads>,
        ) -> Result<String, GatewayError> {
            let dependents = gateway
                .who_needs(
                    vec![source.into()],
                    vec![Platform::Linux64],
                    PackageName::from_str("bors").unwrap(),
                )
                .with_reporter(reporter.clone())
                .execute()
                .await?;
            Ok(render(&dependents))
        }

        /// Runs an ordinary, non-recursive query for `foo` on `channel`.
        async fn query_foo(gateway: &Gateway, channel: &Channel, reporter: &Arc<Downloads>) {
            let result = gateway
                .query(
                    vec![channel.clone()],
                    vec![Platform::Linux64],
                    vec![PackageName::from_str("foo").unwrap()],
                )
                .recursive(false)
                .with_reporter(reporter.clone())
                .execute()
                .await
                .unwrap();
            assert_eq!(
                result
                    .repodata
                    .iter()
                    .map(crate::gateway::RepoData::len)
                    .sum::<usize>(),
                1
            );
        }

        /// The number of package names held in the per-name record cache of
        /// the scan subdir of `channel`, if one was built.
        fn scan_subdir_cached_packages(gateway: &Gateway, channel: &Channel) -> Option<usize> {
            let subdir = gateway
                .inner
                .scan_subdirs
                .get(&(channel.clone(), Platform::Linux64))?;
            let super::super::Subdir::Found(data) = subdir.as_ref() else {
                panic!("expected the scan subdir to exist");
            };
            Some(data.cached_package_count())
        }

        /// A default, sharding-enabled gateway reads the full repodata for
        /// `who_needs` when the channel offers both, and keeps the subdir it
        /// builds out of the cache ordinary queries use.
        #[tokio::test]
        async fn test_who_needs_prefers_full_repodata() {
            let channel = TestChannel::serve(FULL_AND_SHARDED).await;
            let cache_dir = tempfile::tempdir().unwrap();
            let gateway = uncached_gateway(cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            let dependents = who_needs_bors(&gateway, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);

            assert_eq!(downloads.full(), 1, "one full repodata download");
            assert_eq!(downloads.indexes(), 0, "no shard index download");
            assert_eq!(downloads.shards(), 0, "no shard downloads");

            // The subdir lives in the scan cache only, and the scan did not
            // fill its per-name record cache.
            assert_eq!(gateway.inner.subdirs.len(), 0);
            assert_eq!(gateway.inner.scan_subdirs.len(), 1);
            assert_eq!(
                scan_subdir_cached_packages(&gateway, &channel.channel()),
                Some(0)
            );
        }

        /// A sharded subdir cached by an earlier ordinary query does not
        /// serve `who_needs`; the scan still reads the full repodata.
        #[tokio::test]
        async fn test_query_then_who_needs_uses_sharded_then_full() {
            let channel = TestChannel::serve(FULL_AND_SHARDED).await;
            let cache_dir = tempfile::tempdir().unwrap();
            let gateway = uncached_gateway(cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            query_foo(&gateway, &channel.channel(), &downloads).await;
            assert_eq!(downloads.indexes(), 1, "the query reads the shard index");
            assert_eq!(downloads.shards(), 1, "the query reads foo's shard");
            assert_eq!(downloads.full(), 0);
            downloads.clear();

            let dependents = who_needs_bors(&gateway, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);
            assert_eq!(downloads.full(), 1, "the scan reads the full repodata");
            assert_eq!(downloads.indexes(), 0);
            assert_eq!(
                downloads.shards(),
                0,
                "the scan does not reuse the sharded subdir"
            );

            assert_eq!(gateway.inner.subdirs.len(), 1);
            assert_eq!(gateway.inner.scan_subdirs.len(), 1);
        }

        /// A full-repodata subdir built for `who_needs` does not serve later
        /// ordinary queries; they still read sharded repodata.
        #[tokio::test]
        async fn test_who_needs_then_query_uses_full_then_sharded() {
            let channel = TestChannel::serve(FULL_AND_SHARDED).await;
            let cache_dir = tempfile::tempdir().unwrap();
            let gateway = uncached_gateway(cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            let dependents = who_needs_bors(&gateway, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);
            assert_eq!(downloads.full(), 1);
            assert_eq!(downloads.indexes(), 0);
            downloads.clear();

            query_foo(&gateway, &channel.channel(), &downloads).await;
            assert_eq!(downloads.indexes(), 1, "the query reads the shard index");
            assert_eq!(downloads.shards(), 1, "the query reads foo's shard");
            assert_eq!(
                downloads.full(),
                0,
                "the query does not reuse the scan subdir"
            );

            assert_eq!(gateway.inner.subdirs.len(), 1);
            assert_eq!(gateway.inner.scan_subdirs.len(), 1);
        }

        /// Without a full repodata the scan falls back to sharded repodata:
        /// the index plus one shard per package.
        #[tokio::test]
        async fn test_who_needs_falls_back_to_shards() {
            let channel = TestChannel::serve(SHARDED_ONLY).await;
            let cache_dir = tempfile::tempdir().unwrap();
            let gateway = uncached_gateway(cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            let dependents = who_needs_bors(&gateway, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);
            assert_eq!(downloads.indexes(), 1);
            assert_eq!(downloads.shards(), records().len());

            // The sharded fallback still does not fill the record cache.
            assert_eq!(
                scan_subdir_cached_packages(&gateway, &channel.channel()),
                Some(0)
            );
        }

        /// A channel that only serves full repodata works for both kinds of
        /// query, and the ordinary query's failed probe for a shard index
        /// does not disturb the scan.
        #[tokio::test]
        async fn test_who_needs_full_only_channel() {
            let channel = TestChannel::serve(FULL_ONLY).await;
            let cache_dir = tempfile::tempdir().unwrap();
            let gateway = uncached_gateway(cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            let dependents = who_needs_bors(&gateway, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);
            assert_eq!(downloads.full(), 1);
            assert_eq!(downloads.indexes(), 0, "the scan never probes for shards");
            downloads.clear();

            query_foo(&gateway, &channel.channel(), &downloads).await;
            assert_eq!(downloads.full(), 1, "the query falls back to full repodata");
        }

        /// When the gateway disables sharding, `who_needs` shares the
        /// ordinary subdir instead of building one of its own, so the
        /// repodata is read once for both kinds of query.
        #[tokio::test]
        async fn test_who_needs_shares_subdir_when_sharding_disabled() {
            let channel = TestChannel::serve(FULL_AND_SHARDED).await;
            let cache_dir = tempfile::tempdir().unwrap();
            let gateway = gateway(CacheAction::NoCache, false, cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            let dependents = who_needs_bors(&gateway, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);
            assert_eq!(downloads.full(), 1);
            assert_eq!(gateway.inner.subdirs.len(), 1);
            assert_eq!(gateway.inner.scan_subdirs.len(), 0);

            query_foo(&gateway, &channel.channel(), &downloads).await;
            assert_eq!(downloads.full(), 1, "the query reuses the shared subdir");
            assert_eq!(downloads.indexes(), 0);
        }

        /// Concurrent scans of the same subdir are coalesced into a single
        /// fetch of the full repodata.
        #[tokio::test]
        async fn test_concurrent_who_needs_are_coalesced() {
            let channel = TestChannel::serve(FULL_AND_SHARDED).await;
            let cache_dir = tempfile::tempdir().unwrap();
            let gateway = uncached_gateway(cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            let (first, second) = tokio::join!(
                who_needs_bors(&gateway, channel.channel(), &downloads),
                who_needs_bors(&gateway, channel.channel(), &downloads),
            );
            assert_eq!(first.unwrap(), BORS_DEPENDENTS);
            assert_eq!(second.unwrap(), BORS_DEPENDENTS);
            assert_eq!(downloads.full(), 1, "the concurrent scans share one fetch");
            assert_eq!(gateway.inner.scan_subdirs.len(), 1);
        }

        /// Clearing the repodata cache drops the scan subdirs too, so the
        /// next scan fetches the repodata again.
        #[tokio::test]
        async fn test_clear_repodata_cache_clears_scan_subdirs() {
            let channel = TestChannel::serve(FULL_AND_SHARDED).await;
            let cache_dir = tempfile::tempdir().unwrap();
            let gateway = uncached_gateway(cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            who_needs_bors(&gateway, channel.channel(), &downloads)
                .await
                .unwrap();
            who_needs_bors(&gateway, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(downloads.full(), 1, "the second scan reuses the subdir");
            assert_eq!(gateway.inner.scan_subdirs.len(), 1);

            gateway
                .clear_repodata_cache(
                    &channel.channel(),
                    SubdirSelection::default(),
                    CacheClearMode::InMemoryOnly,
                )
                .unwrap();
            assert_eq!(gateway.inner.scan_subdirs.len(), 0);

            let dependents = who_needs_bors(&gateway, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);
            assert_eq!(downloads.full(), 2, "the scan after clearing fetches again");
        }

        /// A cache-only gateway serves `who_needs` from a cached full
        /// repodata without touching the network.
        #[tokio::test]
        async fn test_who_needs_cache_only_uses_cached_full_repodata() {
            let channel = TestChannel::serve(FULL_AND_SHARDED).await;
            let cache_dir = tempfile::tempdir().unwrap();

            // Warm the cache with an online gateway.
            let online = gateway(CacheAction::CacheOrFetch, true, cache_dir.path());
            let downloads = Arc::new(Downloads::default());
            who_needs_bors(&online, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(downloads.full(), 1);

            let offline = gateway(CacheAction::ForceCacheOnly, true, cache_dir.path());
            let downloads = Arc::new(Downloads::default());
            let dependents = who_needs_bors(&offline, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);
            assert_eq!(
                downloads.full() + downloads.indexes() + downloads.shards(),
                0
            );
        }

        /// A cache-only gateway without a cached full repodata falls back to
        /// the cached shards of the channel.
        #[tokio::test]
        async fn test_who_needs_cache_only_falls_back_to_cached_shards() {
            let channel = TestChannel::serve(SHARDED_ONLY).await;
            let cache_dir = tempfile::tempdir().unwrap();

            // Warm the cache with an online gateway; the scan fetches the
            // index and every shard.
            let online = gateway(CacheAction::CacheOrFetch, true, cache_dir.path());
            let downloads = Arc::new(Downloads::default());
            who_needs_bors(&online, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(downloads.shards(), records().len());

            let offline = gateway(CacheAction::ForceCacheOnly, true, cache_dir.path());
            let downloads = Arc::new(Downloads::default());
            let dependents = who_needs_bors(&offline, channel.channel(), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);
            assert_eq!(
                downloads.full() + downloads.indexes() + downloads.shards(),
                0
            );
        }

        /// A cache-only gateway with nothing cached reports the missing full
        /// repodata, as an unsharded gateway would, rather than the missing
        /// shard index it fell back to.
        #[tokio::test]
        async fn test_who_needs_cache_only_without_cache_fails() {
            let channel = TestChannel::serve(FULL_AND_SHARDED).await;
            let cache_dir = tempfile::tempdir().unwrap();
            let offline = gateway(CacheAction::ForceCacheOnly, true, cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            let err = who_needs_bors(&offline, channel.channel(), &downloads)
                .await
                .unwrap_err();
            assert_matches!(
                err,
                GatewayError::FetchRepoDataError(FetchRepoDataError::NoCacheAvailable(_))
            );
        }

        /// A custom source serving the test records.
        struct TestSource;

        #[async_trait::async_trait]
        impl RepoDataSource for TestSource {
            async fn fetch_package_records(
                &self,
                platform: Platform,
                name: &PackageName,
            ) -> Result<Vec<Arc<RepoDataRecord>>, GatewayError> {
                assert_eq!(platform, Platform::Linux64);
                Ok(records()
                    .into_iter()
                    .filter(|record| &record.name == name)
                    .map(|record| {
                        Arc::new(RepoDataRecord {
                            url: Url::parse("https://example.com/")
                                .unwrap()
                                .join(&identifier(&record).to_file_name())
                                .unwrap(),
                            channel: None,
                            identifier: identifier(&record),
                            package_record: record,
                        })
                    })
                    .collect())
            }

            fn package_names(&self, platform: Platform) -> Vec<String> {
                assert_eq!(platform, Platform::Linux64);
                records()
                    .iter()
                    .map(|record| record.name.as_source().to_string())
                    .collect()
            }
        }

        /// Custom and sparse sources are scanned as they are: no subdir is
        /// built for them and nothing is downloaded.
        #[tokio::test]
        async fn test_custom_and_sparse_sources_are_unchanged() {
            let cache_dir = tempfile::tempdir().unwrap();
            let gateway = uncached_gateway(cache_dir.path());
            let downloads = Arc::new(Downloads::default());

            let custom: Arc<dyn RepoDataSource> = Arc::new(TestSource);
            let dependents = who_needs_bors(&gateway, Source::Custom(custom), &downloads)
                .await
                .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);

            let sparse = crate::sparse::SparseRepoData::from_bytes(
                Channel::from_url(Url::parse("https://example.com/channel/").unwrap()),
                SUBDIR,
                full_repodata().into_bytes().into(),
                None,
            )
            .unwrap();
            let dependents = who_needs_bors(
                &gateway,
                Source::SparseRepoData(vec![Arc::new(sparse)]),
                &downloads,
            )
            .await
            .unwrap();
            assert_eq!(dependents, BORS_DEPENDENTS);

            assert_eq!(gateway.inner.subdirs.len(), 0);
            assert_eq!(gateway.inner.scan_subdirs.len(), 0);
            assert_eq!(
                downloads.full() + downloads.indexes() + downloads.shards(),
                0
            );
        }
    }
}
