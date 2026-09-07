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
                    let subdir = gateway
                        .get_or_create_subdir(&channel, platform, reporter)
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
}
