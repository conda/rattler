//! A streaming reverse-dependency query over a [`Gateway`](super::Gateway).
//!
//! Unlike a wildcard [`RepoDataQuery`](super::RepoDataQuery) followed by
//! [`repoquery::who_needs`](crate::repoquery::who_needs), this query scans
//! the records of every package name one name at a time, keeps only the
//! matching records, and never inserts the scanned records into the
//! gateway's long-lived per-name cache. Peak memory is therefore bounded
//! by the in-flight package scans instead of the complete repodata of the
//! queried platforms.

use std::{future::IntoFuture, sync::Arc};

use futures::{StreamExt, stream::FuturesUnordered};
use rattler_conda_types::{Channel, ChannelUrl, PackageName, Platform};

use super::{
    GatewayError, GatewayInner, GatewayWarning,
    channel_expander::{ChannelExpander, ChannelRelationsMode, ChannelRelationsWarning},
    channel_relations::DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH,
    local_subdir::LocalSubdirClient,
    query::{BoxFuture, FetchErrorPolicy, box_future, fetch_subdir_with_policy},
    source::{CustomSourceClient, Source},
    subdir::{Subdir, SubdirData},
};
use crate::{
    Reporter,
    repoquery::{OwnedDependent, WhoNeedsTarget, who_needs_owned},
};

/// How many platforms are queried and scanned concurrently. Bounds peak
/// memory: at most this many platforms are actively scanned at the same
/// time.
const PLATFORM_CONCURRENCY: usize = 4;

/// How many package names one batch task fetches and scans sequentially.
/// Batching amortizes the per-task overhead over many cheap per-name
/// fetches.
const NAME_BATCH_SIZE: usize = 500;

/// How many batch tasks run concurrently within one subdir. Together with
/// [`NAME_BATCH_SIZE`] this bounds peak memory to the records of one
/// package per in-flight batch, per active platform, while spreading the
/// scan over the runtime's worker threads.
const BATCH_CONCURRENCY: usize = 8;

/// Result of a successful [`WhoNeedsQuery::execute`].
#[derive(Debug, Default)]
pub struct WhoNeedsQueryOutput {
    /// The records that reference the queried package, grouped by the input
    /// platform order. Within a platform the order is unspecified.
    pub dependents: Vec<OwnedDependent>,

    /// Non-fatal warnings encountered during the query. Also streamed to
    /// [`Reporter::on_gateway_warning`] as they are recorded.
    pub warnings: Vec<GatewayWarning>,
}

/// A reverse-dependency query created through
/// [`Gateway::who_needs`](super::Gateway::who_needs).
///
/// When executed, every package of the queried sources and platforms is
/// scanned against the target. Records of scanned packages are dropped as
/// soon as the package is processed and are *not* inserted into the
/// gateway's per-name record cache (previously cached entries are still
/// reused), so a query over a large channel does not permanently grow the
/// gateway's memory footprint. Only the matching records are retained,
/// shared via `Arc` in the returned [`OwnedDependent`]s.
///
/// Platforms are scanned independently with bounded concurrency; the
/// result keeps the input platform order (duplicate platforms are scanned
/// once). See [`repoquery::who_needs`](crate::repoquery::who_needs) for
/// the matching semantics of the different [`WhoNeedsTarget`] variants.
#[derive(Clone)]
pub struct WhoNeedsQuery {
    gateway: Arc<GatewayInner>,
    sources: Vec<Source>,
    platforms: Vec<Platform>,
    target: WhoNeedsTarget,
    reporter: Option<Arc<dyn Reporter>>,
    channel_relations_mode: ChannelRelationsMode,
    channel_relations_max_depth: usize,
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
            channel_relations_mode: ChannelRelationsMode::default(),
            channel_relations_max_depth: DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH,
        }
    }

    /// How to treat CEP-42 `channel_relations`. Defaults to
    /// [`ChannelRelationsMode::Warn`].
    #[must_use]
    pub fn channel_relations(self, mode: ChannelRelationsMode) -> Self {
        Self {
            channel_relations_mode: mode,
            ..self
        }
    }

    /// Maximum CEP-42 recursion depth. Defaults to
    /// [`DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH`](super::DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH).
    #[must_use]
    pub fn channel_relations_max_depth(self, depth: usize) -> Self {
        Self {
            channel_relations_max_depth: depth,
            ..self
        }
    }

    /// Sets the reporter to use for this query.
    pub fn with_reporter(self, reporter: impl Reporter + 'static) -> Self {
        Self {
            reporter: Some(Arc::new(reporter)),
            ..self
        }
    }

    /// Execute the query and return the reverse dependencies of the target
    /// along with any non-fatal warnings.
    pub async fn execute(self) -> Result<WhoNeedsQueryOutput, GatewayError> {
        // Deduplicate platforms while keeping the input order, so the
        // result order stays deterministic and no subdir is scanned twice.
        let mut seen_platforms = std::collections::HashSet::new();
        let platforms: Vec<Platform> = self
            .platforms
            .iter()
            .copied()
            .filter(|p| seen_platforms.insert(*p))
            .collect();

        // Scan platforms independently with bounded concurrency.
        // `buffer_unordered` starts the next platform as soon as any
        // in-flight one finishes; results carry the input platform index
        // and are re-sorted afterwards so the output order stays
        // deterministic.
        let mut per_platform = futures::stream::iter(platforms.into_iter().enumerate().map(
            |(platform_index, platform)| {
                let scan = scan_platform(
                    self.gateway.clone(),
                    self.sources.clone(),
                    platform,
                    self.target.clone(),
                    self.channel_relations_mode,
                    self.channel_relations_max_depth,
                    self.reporter.clone(),
                );
                async move { spawn_scan(scan).await.map(|ok| (platform_index, ok)) }
            },
        ))
        .buffer_unordered(PLATFORM_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, GatewayError>>()?;

        // Restore the input platform order.
        per_platform.sort_unstable_by_key(|(platform_index, _)| *platform_index);

        let mut output = WhoNeedsQueryOutput::default();
        for (_, (dependents, warnings)) in per_platform {
            output.dependents.extend(dependents);
            output.warnings.extend(warnings);
        }
        Ok(output)
    }
}

impl IntoFuture for WhoNeedsQuery {
    type Output = Result<WhoNeedsQueryOutput, GatewayError>;
    type IntoFuture = BoxFuture<Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        box_future(self.execute())
    }
}

/// Runs a platform scan on its own tokio task so scans of different
/// platforms run in parallel on the runtime's worker threads instead of
/// interleaving on the task driving the query.
#[cfg(not(target_arch = "wasm32"))]
async fn spawn_scan<T: Send + 'static>(
    scan: impl Future<Output = Result<T, GatewayError>> + Send + 'static,
) -> Result<T, GatewayError> {
    tokio::spawn(scan)
        .await
        .expect("the platform scan task panicked")
}

/// On wasm there are no threads to parallelize over; run the scan in place.
#[cfg(target_arch = "wasm32")]
async fn spawn_scan<T>(
    scan: impl Future<Output = Result<T, GatewayError>>,
) -> Result<T, GatewayError> {
    scan.await
}

type PlatformScanResult = Result<(Vec<OwnedDependent>, Vec<GatewayWarning>), GatewayError>;

/// A resolved subdir tagged with the index of the source that introduced
/// it, plus an optional swallowed fetch warning.
type PendingSubdir = (
    usize,
    ChannelUrl,
    Arc<Subdir>,
    Option<ChannelRelationsWarning>,
);

/// Resolves the subdirs of every source for `platform` (following CEP-42
/// channel relations like the other gateway queries) and scans them
/// package by package.
async fn scan_platform(
    gateway: Arc<GatewayInner>,
    sources: Vec<Source>,
    platform: Platform,
    target: WhoNeedsTarget,
    channel_relations_mode: ChannelRelationsMode,
    channel_relations_max_depth: usize,
    reporter: Option<Arc<dyn Reporter>>,
) -> PlatformScanResult {
    let mut expander = ChannelExpander::new(
        channel_relations_mode,
        channel_relations_max_depth,
        vec![platform],
        reporter.clone(),
    );

    // Kick off the subdir fetch of every channel source; custom and sparse
    // sources resolve immediately. Each resolved subdir is tagged with the
    // index of the source that introduced it (CEP-42-discovered channels
    // inherit the index of the declaring channel) so the scan order follows
    // the caller's source order.
    let mut pending: FuturesUnordered<BoxFuture<Result<PendingSubdir, GatewayError>>> =
        FuturesUnordered::new();
    let mut subdirs: Vec<(usize, Arc<Subdir>)> = Vec::new();

    for (source_index, source) in sources.into_iter().enumerate() {
        match source {
            Source::Channel(channel) => {
                let (url, channel) = expander.register_user_channel(channel);
                pending.push(subdir_fetch_future(
                    &gateway,
                    channel,
                    platform,
                    url,
                    reporter.clone(),
                    FetchErrorPolicy::Propagate,
                    source_index,
                ));
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

    let discovered_policy = if expander.strict() {
        FetchErrorPolicy::WrapAsChannelRelationsError
    } else {
        FetchErrorPolicy::SwallowAsWarning
    };
    while let Some(result) = pending.next().await {
        let (source_index, url, subdir, warning) = result?;
        if let Some(warning) = warning {
            expander.push_warning(warning);
        }
        for (new_url, new_channel, new_platform) in expander.observe(&url, platform, &subdir)? {
            pending.push(subdir_fetch_future(
                &gateway,
                new_channel,
                new_platform,
                new_url,
                reporter.clone(),
                discovered_policy,
                source_index,
            ));
        }
        subdirs.push((source_index, subdir));
    }
    if expander.enabled() && expander.has_observed_relations() {
        expander.finalize()?;
    }
    let warnings = expander
        .take_warnings()
        .into_iter()
        .map(GatewayWarning::from)
        .collect();

    subdirs.sort_by_key(|(source_index, _)| *source_index);

    let mut dependents = Vec::new();
    for (_, subdir) in &subdirs {
        scan_subdir(subdir.clone(), &target, reporter.clone(), &mut dependents).await?;
    }
    Ok((dependents, warnings))
}

/// Builds a future that fetches a channel subdir and tags it with the
/// source index it belongs to.
fn subdir_fetch_future(
    gateway: &Arc<GatewayInner>,
    channel: Arc<Channel>,
    platform: Platform,
    url: ChannelUrl,
    reporter: Option<Arc<dyn Reporter>>,
    policy: FetchErrorPolicy,
    source_index: usize,
) -> BoxFuture<Result<PendingSubdir, GatewayError>> {
    let gateway = gateway.clone();
    box_future(async move {
        let (subdir, warning) =
            fetch_subdir_with_policy(&gateway, &channel, platform, &url, reporter, policy).await?;
        Ok((source_index, url, subdir, warning))
    })
}

/// Scans every package of `subdir` against `target` in batches of
/// [`NAME_BATCH_SIZE`] names. Each batch runs as its own task (see
/// [`spawn_scan`]) that fetches one package at a time, keeps the matches,
/// and drops the scanned records before fetching the next package. The
/// scanned records are never inserted into the subdir's per-name record
/// cache.
async fn scan_subdir(
    subdir: Arc<Subdir>,
    target: &WhoNeedsTarget,
    reporter: Option<Arc<dyn Reporter>>,
    dependents: &mut Vec<OwnedDependent>,
) -> Result<(), GatewayError> {
    let Subdir::Found(subdir_data) = subdir.as_ref() else {
        return Ok(());
    };
    let names: Vec<PackageName> = subdir_data
        .package_names()
        .into_iter()
        .filter_map(|name| PackageName::try_from(name).ok())
        .collect();
    let batches: Vec<Vec<PackageName>> = names
        .chunks(NAME_BATCH_SIZE)
        .map(<[PackageName]>::to_vec)
        .collect();

    let mut batch_results = futures::stream::iter(batches.into_iter().map(|batch| {
        let subdir = subdir.clone();
        let target = target.clone();
        let reporter = reporter.clone();
        async move {
            spawn_scan(async move {
                let Subdir::Found(subdir_data) = subdir.as_ref() else {
                    return Ok(Vec::new());
                };
                let mut matches = Vec::new();
                for name in batch {
                    let records = subdir_data
                        .fetch_package_records_uncached(&name, reporter.as_deref())
                        .await?;
                    matches.extend(who_needs_owned(&records, &target));
                    // The scanned records are dropped here; only the
                    // matches survive.
                }
                Ok(matches)
            })
            .await
        }
    }))
    .buffer_unordered(BATCH_CONCURRENCY);

    while let Some(matches) = batch_results.next().await {
        dependents.extend(matches?);
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::{path::Path, str::FromStr};

    use rattler_conda_types::{Channel, MatchSpec, PackageName, ParseMatchSpecOptions, Platform};

    use super::super::Gateway;
    use crate::repoquery::{self, WhoNeedsTarget};

    async fn local_conda_forge() -> Channel {
        tokio::try_join!(
            tools::fetch_test_conda_forge_repodata_async("noarch"),
            tools::fetch_test_conda_forge_repodata_async("linux-64")
        )
        .unwrap();
        Channel::try_from_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/channels/conda-forge"),
        )
        .unwrap()
    }

    /// A sortable identity of a dependent for comparisons.
    fn key(record_url: &url::Url, dependency: &str) -> (String, String) {
        (record_url.to_string(), dependency.to_string())
    }

    /// Computes the reverse dependencies the pre-existing way: a wildcard
    /// query that materializes everything, followed by the pure
    /// `repoquery::who_needs`.
    async fn materialized_who_needs(
        gateway: &Gateway,
        channel: &Channel,
        platforms: Vec<Platform>,
        target: WhoNeedsTarget,
    ) -> Vec<(String, String)> {
        let wildcard = MatchSpec::from_str(
            "*",
            ParseMatchSpecOptions::strict().with_exact_names_only(false),
        )
        .unwrap();
        let output = gateway
            .query(vec![channel.clone()], platforms, vec![wildcard])
            .recursive(false)
            .execute()
            .await
            .unwrap();
        let mut result: Vec<(String, String)> = output
            .repodata
            .iter()
            .flat_map(|repodata| repoquery::who_needs(repodata.iter(), target.clone()))
            .map(|dependent| key(&dependent.record.url, dependent.dependency))
            .collect();
        result.sort();
        result
    }

    #[tokio::test]
    async fn test_who_needs_matches_materialized_query() {
        let channel = local_conda_forge().await;
        let gateway = Gateway::new();
        let platforms = vec![Platform::Linux64, Platform::NoArch];

        // Name target.
        let name_target = WhoNeedsTarget::from(PackageName::from_str("python_abi").unwrap());
        let output = gateway
            .who_needs(
                vec![channel.clone()],
                platforms.clone(),
                name_target.clone(),
            )
            .execute()
            .await
            .unwrap();
        assert!(output.warnings.is_empty());
        assert!(!output.dependents.is_empty());
        let mut streaming: Vec<_> = output
            .dependents
            .iter()
            .map(|dependent| key(&dependent.record.url, &dependent.dependency))
            .collect();
        streaming.sort();
        assert_eq!(
            streaming,
            materialized_who_needs(&gateway, &channel, platforms.clone(), name_target).await
        );

        // Concrete record target: take some record from the channel.
        let record = gateway
            .query(
                vec![channel.clone()],
                platforms.clone(),
                vec![PackageName::from_str("python").unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .unwrap()
            .repodata
            .iter()
            .flat_map(|repodata| repodata.iter())
            .next()
            .expect("the test channel contains python records")
            .clone();
        let record_target = WhoNeedsTarget::from(record.package_record);
        let output = gateway
            .who_needs(
                vec![channel.clone()],
                platforms.clone(),
                record_target.clone(),
            )
            .execute()
            .await
            .unwrap();
        let mut streaming: Vec<_> = output
            .dependents
            .iter()
            .map(|dependent| key(&dependent.record.url, &dependent.dependency))
            .collect();
        streaming.sort();
        assert_eq!(
            streaming,
            materialized_who_needs(&gateway, &channel, platforms, record_target).await
        );
    }

    #[tokio::test]
    async fn test_who_needs_platform_order_and_duplicates() {
        let channel = local_conda_forge().await;
        let gateway = Gateway::new();
        let target = WhoNeedsTarget::from(PackageName::from_str("python_abi").unwrap());

        let output = gateway
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
        let subdirs: Vec<&str> = output
            .dependents
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
        assert_eq!(output.dependents.len(), deduplicated.dependents.len());
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
        let output = gateway
            .who_needs(
                vec![channel.clone()],
                vec![Platform::Linux64, Platform::NoArch],
                PackageName::from_str("python_abi").unwrap(),
            )
            .execute()
            .await
            .unwrap();
        assert!(!output.dependents.is_empty());

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
