//! A streaming reverse-dependency query over a [`Gateway`](super::Gateway).
//!
//! Rather than materializing the repodata of the queried platforms (as a
//! wildcard [`RepoDataQuery`](super::RepoDataQuery) would), this query
//! scans the records of every package name one name at a time, keeps only
//! the matching records, and never inserts the scanned records into the
//! gateway's long-lived per-name cache. Peak memory is therefore bounded
//! by the in-flight package scans instead of the complete repodata of the
//! queried platforms.

use std::{future::IntoFuture, sync::Arc};

use futures::{StreamExt, stream::FuturesUnordered};
use rattler_conda_types::{PackageName, Platform};

use super::{
    GatewayError, GatewayInner,
    local_subdir::LocalSubdirClient,
    query::{BoxFuture, box_future},
    source::{CustomSourceClient, Source},
    subdir::{Subdir, SubdirData},
};
use crate::{
    Reporter,
    repoquery::{Dependent, WhoNeedsTarget, who_needs},
};

/// How many package names one batch task fetches and scans sequentially.
/// Batching amortizes the per-task overhead over many cheap per-name
/// fetches.
const NAME_BATCH_SIZE: usize = 500;

/// Result of a successful [`WhoNeedsQuery::execute`].
#[derive(Debug, Default)]
pub struct WhoNeedsQueryOutput {
    /// The records that reference the queried package, grouped by the input
    /// platform order. Within a platform the order is unspecified.
    pub dependents: Vec<Dependent>,
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
/// shared via `Arc` in the returned [`Dependent`]s.
///
/// Platforms are scanned independently and concurrently; the result keeps
/// the input platform order (duplicate platforms are scanned once). See
/// [`WhoNeedsTarget`] for the matching semantics of its variants.
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

    /// Execute the query and return the reverse dependencies of the target.
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

        // Scan every platform concurrently. Results carry the input
        // platform index and are re-sorted afterwards so the output order
        // stays deterministic regardless of completion order.
        let mut per_platform = platforms
            .into_iter()
            .enumerate()
            .map(|(platform_index, platform)| {
                let scan = scan_platform(
                    self.gateway.clone(),
                    self.sources.clone(),
                    platform,
                    self.target.clone(),
                    self.reporter.clone(),
                );
                async move { spawn_scan(scan).await.map(|ok| (platform_index, ok)) }
            })
            .collect::<FuturesUnordered<_>>()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, GatewayError>>()?;

        // Restore the input platform order.
        per_platform.sort_unstable_by_key(|(platform_index, _)| *platform_index);

        let mut output = WhoNeedsQueryOutput::default();
        for (_, dependents) in per_platform {
            output.dependents.extend(dependents);
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

/// A resolved subdir tagged with the index of the source it came from.
type IndexedSubdir = (usize, Arc<Subdir>);

/// Resolves the subdirs of every source for `platform` and scans them
/// package by package, in the caller's source order.
async fn scan_platform(
    gateway: Arc<GatewayInner>,
    sources: Vec<Source>,
    platform: Platform,
    target: WhoNeedsTarget,
    reporter: Option<Arc<dyn Reporter>>,
) -> Result<Vec<Dependent>, GatewayError> {
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

    let mut dependents = Vec::new();
    for (_, subdir) in &subdirs {
        scan_subdir(subdir.clone(), &target, reporter.clone(), &mut dependents).await?;
    }
    Ok(dependents)
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
    dependents: &mut Vec<Dependent>,
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

    let mut batch_results = batches
        .into_iter()
        .map(|batch| {
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
        .collect::<FuturesUnordered<_>>();

    while let Some(matches) = batch_results.next().await {
        dependents.extend(matches?);
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::{path::Path, str::FromStr};

    use rattler_conda_types::{Channel, PackageName, Platform};

    use super::super::Gateway;
    use crate::repoquery::{DependencyKind, Dependent, WhoNeedsTarget};

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
        let output = Gateway::new()
            .who_needs(vec![local_channel(channel)], vec![platform], target)
            .execute()
            .await
            .unwrap();
        render(&output.dependents)
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
