use std::{
    collections::{BTreeSet, HashMap, HashSet},
    future::IntoFuture,
    sync::Arc,
};

use futures::{StreamExt, select_biased, stream::FuturesUnordered};
use rattler_conda_types::{
    Channel, ChannelUrl, MatchSpec, Matches, PackageName, PackageNameMatcher, RepoDataRecord,
    Subdir, referenced_virtual_packages,
};
use url::Url;

use super::{
    AcceptedDetectorRegistration, BarrierCell, ChannelNoticeResult, GatewayError, GatewayInner,
    GatewayWarning, RejectedDetectorRegistration, RepoData, VirtualPackageDetectorsQuery,
    boxed::{BoxFuture, box_future},
    channel_expander::{ChannelRelationsMode, ChannelRelationsWarning},
    channel_expansion::{ChannelDiscovery, ScheduledSubdir},
    channel_relations::DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH,
    local_subdir::LocalSubdirClient,
    source::{CustomSourceClient, ExpandedSource, Source, SourcePosition},
    subdir::{PackageRecords, SubdirData, SubdirState, extract_unique_deps_split},
};
use crate::Reporter;

type RecordPatch = dyn Fn(&RepoDataRecord) -> Option<RepoDataRecord> + Send + Sync;

/// Result of a successful [`RepoDataQuery::execute`].
///
/// Implements [`Deref<Target = [RepoData]>`](std::ops::Deref) and
/// [`IntoIterator`], so call sites that only need the records can use
/// it like a `Vec<RepoData>`.
#[derive(Debug, Default)]
pub struct RepoDataQueryOutput {
    /// One bucket per source and platform, where every channel of a
    /// multichannel is a separate source. CEP-42-discovered channels are
    /// inserted next to the channel that introduced them; caller-supplied
    /// sources keep their positions.
    pub repodata: Vec<RepoData>,
    /// CEP-6 notices published by the queried channels. Also streamed to
    /// [`Reporter::on_channel_notice`].
    pub notices: Vec<ChannelNoticeResult>,
    /// Non-fatal warnings encountered during the query. Also streamed
    /// to [`Reporter::on_gateway_warning`] as they are recorded.
    pub warnings: Vec<GatewayWarning>,
    /// Registration ownership and candidate-wide demand, when discovery was enabled.
    pub virtual_package_detectors: Option<QueryVirtualPackageDetectors>,
}

/// Detector metadata for one solve target, without resolving or running detectors.
#[derive(Debug)]
pub struct QueryVirtualPackageDetectors {
    /// The solve target whose registrations were combined with `noarch`.
    pub target_platform: Subdir,
    /// Names referenced by candidate records, input specs, or explicit constraints.
    pub wanted_names: BTreeSet<PackageName>,
    /// All accepted registrations, including registrations outside the wanted set.
    pub registrations: Vec<AcceptedDetectorRegistration>,
    /// Registrations rejected because higher-priority registrations reserved their names.
    pub rejected: Vec<RejectedDetectorRegistration>,
}

impl std::ops::Deref for RepoDataQueryOutput {
    type Target = [RepoData];

    fn deref(&self) -> &[RepoData] {
        &self.repodata
    }
}

impl IntoIterator for RepoDataQueryOutput {
    type Item = RepoData;
    type IntoIter = std::vec::IntoIter<RepoData>;

    fn into_iter(self) -> Self::IntoIter {
        self.repodata.into_iter()
    }
}

impl<'a> IntoIterator for &'a RepoDataQueryOutput {
    type Item = &'a RepoData;
    type IntoIter = std::slice::Iter<'a, RepoData>;

    fn into_iter(self) -> Self::IntoIter {
        self.repodata.iter()
    }
}

/// Result of a successful [`NamesQuery::execute`].
///
/// Implements [`Deref<Target = [PackageName]>`](std::ops::Deref) and
/// [`IntoIterator`], so call sites that only need the names can use
/// it like a `Vec<PackageName>`.
#[derive(Debug, Default)]
pub struct NamesQueryOutput {
    /// Distinct package names contributed by all queried subdirs.
    pub names: Vec<PackageName>,
    /// CEP-6 notices published by the queried channels. Also streamed to
    /// [`Reporter::on_channel_notice`].
    pub notices: Vec<ChannelNoticeResult>,
    /// Non-fatal warnings encountered during the query. Also streamed
    /// to [`Reporter::on_gateway_warning`] as they are recorded.
    pub warnings: Vec<GatewayWarning>,
}

impl std::ops::Deref for NamesQueryOutput {
    type Target = [PackageName];

    fn deref(&self) -> &[PackageName] {
        &self.names
    }
}

impl IntoIterator for NamesQueryOutput {
    type Item = PackageName;
    type IntoIter = std::vec::IntoIter<PackageName>;

    fn into_iter(self) -> Self::IntoIter {
        self.names.into_iter()
    }
}

impl<'a> IntoIterator for &'a NamesQueryOutput {
    type Item = &'a PackageName;
    type IntoIter = std::slice::Iter<'a, PackageName>;

    fn into_iter(self) -> Self::IntoIter {
        self.names.iter()
    }
}

/// Represents a query to execute with a [`Gateway`](super::Gateway).
///
/// When executed the query will asynchronously load the repodata from all
/// subdirectories (combination of sources and platforms).
///
/// Most processing will happen on the background so downloading and parsing
/// can happen simultaneously.
///
/// Repodata is cached by the [`Gateway`](super::Gateway) so executing the
/// same query twice with the same sources will not result in the repodata
/// being fetched twice.
#[derive(Clone)]
pub struct RepoDataQuery {
    /// The gateway that manages all resources
    gateway: Arc<GatewayInner>,

    /// The sources to fetch from (channels or custom sources)
    sources: Vec<Source>,

    /// The platforms the fetch from
    platforms: Vec<Subdir>,

    /// The specs to fetch records for
    specs: Vec<MatchSpec>,

    /// Whether to recursively fetch dependencies
    recursive: bool,

    /// A query-local patch applied to repodata records.
    record_patch: Option<Arc<RecordPatch>>,

    /// The reporter to use by the query.
    reporter: Option<Arc<dyn Reporter>>,

    /// Whether to fetch CEP-6 notices for this query.
    channel_notices: bool,

    /// CEP-42 channel relations handling mode.
    channel_relations_mode: ChannelRelationsMode,

    /// Maximum recursion depth when following CEP-42 `channel_relations`.
    channel_relations_max_depth: usize,

    detector_target: Option<Subdir>,
    detector_constraints: Vec<MatchSpec>,
}

/// Tracks whether specs came from user input or transitive dependencies.
#[derive(Clone)]
enum SourceSpecs {
    /// The record is required by the user.
    Input(Vec<MatchSpec>),

    /// The record is required by a dependency.
    Transitive,
}

/// A request to fetch records for a single package name. The active extras
/// set for the name lives on `QueryExecutor::active_extras`; this struct only
/// carries the spec source and the name (so the executor can look extras up
/// when records arrive).
#[derive(Clone)]
struct PendingRequest {
    name: PackageName,
    specs: SourceSpecs,
}

/// Records cached for a single package name across one or more subdirs.
/// Used to re-walk extras whose activation happens after the first arrival
/// of records for the name.
struct FetchedEntry {
    pkgs: Vec<PackageRecords>,
    /// Spec source captured on first arrival. Used by the late-walk path so
    /// Transitive and Input names follow the same filtering rules they did on
    /// initial walk.
    source: SourceSpecs,
}

/// A spec that references a package by direct URL.
struct DirectUrlSpec {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    spec: MatchSpec,
    url: Url,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    name: PackageName,
}

/// Subdirectory slot: its in-flight fetch barrier, source-kind
/// metadata, and the accumulated records.
struct SubdirHandle {
    barrier: Arc<BarrierCell<Arc<SubdirState>>>,
    kind: SubdirKind,
    data: RepoData,
    /// Position in the caller's `sources` list; `None` for transitively
    /// discovered channels. Anchors the finalize sort so caller
    /// sources keep their positions.
    position: Option<SourcePosition>,
}

/// Origin of a [`SubdirHandle`]; drives final-result reordering.
#[derive(Clone)]
enum SubdirKind {
    /// Channel subdirectory; `url` is the canonical base URL used as
    /// the CEP-42 resolver's identifier.
    Channel { url: ChannelUrl, platform: Subdir },
    /// Custom source; not subject to CEP-42 ordering.
    Custom,
}

/// Where a bucket lands relative to the caller source it is anchored to.
///
/// A discovered channel is placed around the whole caller source, so for a
/// multichannel it outranks or is outranked by all members at once: the
/// members share a priority tier and cannot be split.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Placement {
    /// A discovered channel that outranks the channel that introduced it.
    BeforeSource,
    /// A caller-supplied source, or a member of a caller-supplied group.
    Source,
    /// A discovered channel that the channel that introduced it outranks.
    AfterSource,
}

/// The key the final buckets are sorted by when CEP-42 relations were
/// observed. Fields compare in declaration order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BucketOrder {
    /// Index of the caller source the bucket is anchored to.
    anchor: usize,
    placement: Placement,
    /// Member index for caller sources, CEP-42 priority for discovered
    /// channels.
    priority: usize,
    platform: usize,
    original_index: usize,
}

fn channel_placement(
    url: &ChannelUrl,
    positions: &HashMap<ChannelUrl, SourcePosition>,
    priorities: &HashMap<&ChannelUrl, usize>,
    anchors: &HashMap<ChannelUrl, ChannelUrl>,
) -> (usize, Placement, usize) {
    if let Some(position) = positions.get(url) {
        return (position.source, Placement::Source, position.member);
    }
    let priority = priorities.get(url).copied().unwrap_or(usize::MAX);
    let introduced_by = anchors
        .get(url)
        .and_then(|introducing| Some((introducing, positions.get(introducing)?)));
    match introduced_by {
        Some((introducing, position)) => {
            let introducing_priority = priorities.get(introducing).copied().unwrap_or(usize::MAX);
            let placement = if priority < introducing_priority {
                Placement::BeforeSource
            } else {
                Placement::AfterSource
            };
            (position.source, placement, priority)
        }
        None => (usize::MAX, Placement::AfterSource, priority),
    }
}

/// Where a fetched batch of records should land.
#[derive(Clone, Copy, Debug)]
enum AccumulateTarget {
    // Only constructed by `spawn_direct_url_fetches` which is non-wasm.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    DirectUrl,
    SubdirIndex(usize),
}

impl RepoDataQuery {
    /// Constructs a new instance. This should not be called directly, use
    /// [`Gateway::query`] instead.
    pub(super) fn new(
        gateway: Arc<GatewayInner>,
        sources: Vec<Source>,
        platforms: Vec<Subdir>,
        specs: Vec<MatchSpec>,
    ) -> Self {
        Self {
            gateway,
            sources,
            platforms,
            specs,

            recursive: false,
            record_patch: None,
            reporter: None,
            channel_notices: false,
            channel_relations_mode: ChannelRelationsMode::default(),
            channel_relations_max_depth: DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH,
            detector_target: None,
            detector_constraints: Vec::new(),
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
    /// No effect when the mode is [`ChannelRelationsMode::Disabled`].
    #[must_use]
    pub fn channel_relations_max_depth(self, depth: usize) -> Self {
        Self {
            channel_relations_max_depth: depth,
            ..self
        }
    }

    /// Enable or disable fetching CEP-6 channel notices. Disabled by default.
    #[must_use]
    pub fn channel_notices(self, enabled: bool) -> Self {
        Self {
            channel_notices: enabled,
            ..self
        }
    }

    /// Includes detector registrations and demand for `target` plus `noarch`.
    ///
    /// Missing registration metadata is fetched without adding package records
    /// from unrequested subdirs. Discovery never resolves, installs, or runs a detector.
    #[must_use]
    pub fn virtual_package_detectors(self, target: Subdir) -> Self {
        Self {
            detector_target: Some(target),
            ..self
        }
    }

    /// Adds explicit solver constraints to detector demand.
    ///
    /// These constraints do not fetch packages or filter records. They are only
    /// used when [`Self::virtual_package_detectors`] enables discovery.
    #[must_use]
    pub fn constraints(self, constraints: impl IntoIterator<Item = MatchSpec>) -> Self {
        Self {
            detector_constraints: constraints.into_iter().collect(),
            ..self
        }
    }

    /// Sets whether the query should be recursive. If recursive is set to true
    /// the query will also recursively fetch the dependencies of the packages
    /// that match the root specs.
    ///
    /// Only the dependencies of the records that match the root specs will be
    /// fetched.
    #[must_use]
    pub fn recursive(self, recursive: bool) -> Self {
        Self { recursive, ..self }
    }

    /// Applies a query-local patch to repodata records.
    ///
    /// The patch runs after records are retrieved, including from the gateway
    /// cache, and before recursive dependency discovery. Returning `Some`
    /// replaces the record for this query, while returning `None` reuses the
    /// original record. Replacement records are never written to the gateway
    /// cache. Patches must preserve record identity fields such as the package
    /// name, identifier, and URL.
    #[must_use]
    pub fn with_record_patch(
        self,
        patch: impl Fn(&RepoDataRecord) -> Option<RepoDataRecord> + Send + Sync + 'static,
    ) -> Self {
        Self {
            record_patch: Some(Arc::new(patch)),
            ..self
        }
    }

    /// Sets the reporter to use for this query.
    ///
    /// The reporter is notified of important evens during the execution of the
    /// query. This allows reporting progress back to a user.
    pub fn with_reporter(self, reporter: impl Reporter + 'static) -> Self {
        Self {
            reporter: Some(Arc::new(reporter)),
            ..self
        }
    }

    /// Execute the query and return the resulting repodata records
    /// along with any non-fatal CEP-42 warnings.
    pub async fn execute(self) -> Result<RepoDataQueryOutput, GatewayError> {
        // Short circuit if there are no specs
        if self.specs.is_empty() && self.detector_target.is_none() {
            return Ok(RepoDataQueryOutput::default());
        }

        let executor = QueryExecutor::new(self)?;
        executor.run().await
    }
}

struct DetectorQueryOptions {
    target_platform: Subdir,
    wanted_names: BTreeSet<PackageName>,
    channel_relations_mode: ChannelRelationsMode,
    channel_relations_max_depth: usize,
    channel_positions: HashMap<ChannelUrl, SourcePosition>,
}

impl DetectorQueryOptions {
    fn remember_channel(&mut self, url: &ChannelUrl, position: SourcePosition) {
        if let Some(existing) = self.channel_positions.get_mut(url) {
            *existing = position;
        } else {
            self.channel_positions.insert(url.clone(), position);
        }
    }
}

/// Owns all mutable state during query execution and provides methods for each phase.
struct QueryExecutor {
    // Configuration (immutable after construction)
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    gateway: Arc<GatewayInner>,
    recursive: bool,
    record_patch: Option<Arc<RecordPatch>>,
    reporter: Option<Arc<dyn Reporter>>,
    platforms: Vec<Subdir>,
    detectors: Option<DetectorQueryOptions>,

    // Specs categorized at construction
    direct_url_specs: Vec<DirectUrlSpec>,
    /// `Some` when the query contains direct-URL specs; their records
    /// accumulate here and the bucket is emitted at the head of the
    /// final result.
    direct_url_result: Option<RepoData>,

    /// Specs with glob/regex patterns that need expansion
    pending_pattern_specs: Vec<(PackageNameMatcher, MatchSpec)>,
    /// Track names already considered for pattern expansion (across subdirs)
    pattern_names_seen: HashSet<PackageName>,

    // Mutable state during execution
    /// Normalized (lowercase) package names we've already queued.
    seen: hashbrown::HashMap<String, (), ahash::RandomState>,
    pending_package_specs: ahash::HashMap<PackageName, PendingRequest>,
    /// Every queued name kept around so subdirs that come online
    /// mid-query (via CEP-42 discovery) can still fetch for them.
    all_queued_specs: ahash::HashMap<PackageName, PendingRequest>,
    /// Per-name set of extras that are currently active. Grows monotonically
    /// as new extras are discovered via top-level specs and dep parsing.
    active_extras: ahash::HashMap<PackageName, ahash::HashSet<String>>,
    /// Records cached by name across subdirs. Used to re-walk a name's
    /// records when an extra activates after the first arrival.
    fetched: ahash::HashMap<PackageName, FetchedEntry>,

    // Subdir management; each handle owns its accumulated records.
    subdir_handles: Vec<SubdirHandle>,
    pending_subdirs: FuturesUnordered<BoxFuture<PendingSubdirResult>>,

    // Record fetching
    pending_records: FuturesUnordered<BoxFuture<PendingRecordsResult>>,

    /// Shared incremental channel discovery and subdir fetch scheduling.
    discovery: ChannelDiscovery<'static>,

    /// CEP-6 notice collection state.
    notices: NoticeCollector,
}

/// Collects CEP-6 notices while a query runs. Fetches are queued as channels
/// enter the query — user-supplied and CEP-42-discovered alike — and their
/// futures are driven concurrently with the query's subdir and record
/// fetches. Notice failures are non-fatal by construction:
/// [`GatewayInner::get_channel_notices`] never errors.
struct NoticeCollector {
    /// Whether notice fetching is enabled for the query.
    enabled: bool,
    /// Channels for which a fetch was already queued; guards against
    /// queuing one fetch per platform.
    seen: HashSet<ChannelUrl>,
    /// In-flight notice fetches.
    pending: FuturesUnordered<BoxFuture<Vec<ChannelNoticeResult>>>,
    /// Notices collected so far.
    collected: Vec<ChannelNoticeResult>,
}

impl NoticeCollector {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            seen: HashSet::new(),
            pending: FuturesUnordered::new(),
            collected: Vec::new(),
        }
    }

    /// Queue a notice fetch for `channel` unless notices are disabled or a
    /// fetch for the channel was already queued.
    fn queue(
        &mut self,
        gateway: &Arc<GatewayInner>,
        url: &ChannelUrl,
        channel: Arc<Channel>,
        reporter: Option<Arc<dyn Reporter>>,
    ) {
        if !self.enabled || !self.seen.insert(url.clone()) {
            return;
        }
        let gateway = gateway.clone();
        self.pending.push(box_future(async move {
            gateway
                .get_channel_notices(std::iter::once(channel.as_ref()), reporter.as_deref())
                .await
        }));
    }

    /// Record a completed batch, streaming it to the reporter.
    fn collect(&mut self, reporter: Option<&dyn Reporter>, batch: Vec<ChannelNoticeResult>) {
        GatewayInner::report_channel_notices(reporter, &batch);
        self.collected.extend(batch);
    }
}

impl QueryExecutor {
    /// Construct executor, categorizing specs and initializing subdirs.
    fn new(query: RepoDataQuery) -> Result<Self, GatewayError> {
        // Destructure query to take ownership of all fields
        let RepoDataQuery {
            gateway,
            sources,
            platforms,
            specs,
            recursive,
            record_patch,
            reporter,
            channel_notices,
            channel_relations_mode,
            channel_relations_max_depth,
            detector_target,
            detector_constraints,
        } = query;

        let mut detectors = detector_target.map(|target_platform| DetectorQueryOptions {
            target_platform,
            wanted_names: specs
                .iter()
                .chain(&detector_constraints)
                .filter_map(|spec| spec.name.as_exact())
                .filter(|name| name.as_normalized().starts_with("__"))
                .cloned()
                .collect(),
            channel_relations_mode,
            channel_relations_max_depth,
            channel_positions: HashMap::new(),
        });
        let mut discovery_platforms = platforms.clone();
        if let Some(target) = detector_target {
            for platform in [target, Subdir::NoArch] {
                if !discovery_platforms.contains(&platform) {
                    discovery_platforms.push(platform);
                }
            }
        }

        let mut seen = hashbrown::HashMap::with_hasher(ahash::RandomState::new());
        let mut pending_package_specs: ahash::HashMap<PackageName, PendingRequest> =
            ahash::HashMap::default();
        let mut active_extras: ahash::HashMap<PackageName, ahash::HashSet<String>> =
            ahash::HashMap::default();
        let mut direct_url_specs = Vec::new();
        let mut pending_pattern_specs = Vec::new();
        let pattern_names_seen = HashSet::new();

        // Categorize specs into direct_url_specs, pending_package_specs, and
        // pending_pattern_specs
        for spec in specs {
            if let Some(url) = spec.url.clone() {
                let name = spec.name.clone().into_exact().ok_or(
                    GatewayError::MatchSpecWithoutExactName(Box::new(spec.clone())),
                )?;
                seen.insert(name.as_normalized().to_string(), ());
                if let Some(extras) = spec.extras.as_ref() {
                    active_extras
                        .entry(name.clone())
                        .or_default()
                        .extend(extras.iter().cloned());
                }
                direct_url_specs.push(DirectUrlSpec { spec, url, name });
            } else {
                match &spec.name {
                    PackageNameMatcher::Exact(name) => {
                        seen.insert(name.as_normalized().to_string(), ());
                        if let Some(extras) = spec.extras.as_ref() {
                            active_extras
                                .entry(name.clone())
                                .or_default()
                                .extend(extras.iter().cloned());
                        }
                        let pending =
                            pending_package_specs
                                .entry(name.clone())
                                .or_insert_with(|| PendingRequest {
                                    name: name.clone(),
                                    specs: SourceSpecs::Input(vec![]),
                                });
                        let SourceSpecs::Input(input_specs) = &mut pending.specs else {
                            panic!("SourceSpecs::Input was overwritten by SourceSpecs::Transitive");
                        };
                        input_specs.push(spec);
                    }
                    matcher @ (PackageNameMatcher::Glob(_) | PackageNameMatcher::Regex(_)) => {
                        // Store pattern specs for later expansion
                        pending_pattern_specs.push((matcher.clone(), spec));
                    }
                }
            }
        }

        let direct_url_result = (!direct_url_specs.is_empty()).then(RepoData::default);

        let mut discovery = ChannelDiscovery::new(
            gateway.clone(),
            discovery_platforms,
            channel_relations_mode,
            channel_relations_max_depth,
            reporter.clone(),
            None,
            false,
        );
        if let Some(target) = detector_target {
            for platform in [target, Subdir::NoArch] {
                if !platforms.contains(&platform) {
                    discovery.allow_missing_platform(platform);
                }
            }
        }

        // Iterate per caller-source position then per platform, so each
        // handle remembers where in the caller's `sources` list it came
        // from. The channels of a multichannel share the position of the
        // multichannel, so CEP-42 reordering keeps them together and in
        // the order of the multichannel.
        let sources_with_position = ExpandedSource::expand(sources);
        let total_handles = sources_with_position.len() * platforms.len();
        let mut subdir_handles = Vec::with_capacity(total_handles);
        let pending_subdirs = FuturesUnordered::new();
        let mut notices = NoticeCollector::new(channel_notices);

        if let Some(options) = detectors.as_mut() {
            for (position, source) in &sources_with_position {
                match source {
                    ExpandedSource::Channel(channel, _) => {
                        options.remember_channel(&channel.base_url, *position);
                    }
                    ExpandedSource::SparseRepoData(sparse_list, _) => {
                        for sparse in sparse_list {
                            let url = &sparse.channel.base_url;
                            options.remember_channel(url, *position);
                            for index in 0..discovery.expander.platforms().len() {
                                let platform = discovery.expander.platforms()[index];
                                if platform.as_str() == sparse.subdir() {
                                    discovery.seed_subdir(
                                        url.clone(),
                                        platform,
                                        Arc::new(SubdirState::Found(SubdirData::from_client(
                                            LocalSubdirClient::new(sparse.clone()),
                                        ))),
                                    );
                                }
                            }
                        }
                    }
                    ExpandedSource::Custom(_, _) => {}
                }
            }
        }
        for (position, source) in sources_with_position {
            if detectors.is_some()
                && let ExpandedSource::SparseRepoData(sparse_list, _) = &source
            {
                for sparse in sparse_list {
                    let (url, channel) = discovery.register_user_channel(sparse.channel.clone());
                    for index in 0..discovery.expander.platforms().len() {
                        let platform = discovery.expander.platforms()[index];
                        discovery.schedule_user_subdir(url.clone(), channel.clone(), platform);
                    }
                }
            }
            if let ExpandedSource::Channel(channel, multi_channel) = source {
                let (url, channel) = discovery.register_user_channel(channel);
                notices.queue(&gateway, &url, channel.clone(), reporter.clone());
                for index in 0..discovery.expander.platforms().len() {
                    let platform = discovery.expander.platforms()[index];
                    let scheduled =
                        discovery.schedule_user_subdir(url.clone(), channel.clone(), platform);
                    if !platforms.contains(&platform) {
                        continue;
                    }
                    subdir_handles.push(SubdirHandle {
                        barrier: scheduled.barrier,
                        kind: SubdirKind::Channel {
                            url: scheduled.url,
                            platform,
                        },
                        data: RepoData {
                            multi_channel: multi_channel.clone(),
                            ..RepoData::default()
                        },
                        position: Some(position),
                    });
                }
                continue;
            }
            for &platform in &platforms {
                let source_clone = source.clone();

                let (kind, multi_channel, pending, barrier) = match source_clone {
                    ExpandedSource::Channel(_, _) => {
                        unreachable!("channel sources are scheduled above")
                    }
                    ExpandedSource::Custom(custom_source, multi_channel) => {
                        let barrier = Arc::new(BarrierCell::new());
                        let client = CustomSourceClient::new(custom_source, platform);
                        let subdir = Arc::new(SubdirState::Found(SubdirData::from_client(client)));
                        let b = barrier.clone();
                        let fut = box_future(async move {
                            b.set(subdir.clone()).expect("subdir was set twice");
                            Ok(PendingSubdirOk { subdir })
                        });
                        (SubdirKind::Custom, multi_channel, fut, barrier)
                    }
                    ExpandedSource::SparseRepoData(sparse_list, multi_channel) => {
                        let barrier = Arc::new(BarrierCell::new());
                        // Each entry represents a different subdir, so find the one
                        // matching the requested platform; if none matches, treat it
                        // as having no records, same as a channel that doesn't
                        // publish a given subdir.
                        let matching = sparse_list
                            .iter()
                            .find(|sparse| platform.as_str() == sparse.subdir())
                            .cloned();
                        let url = matching
                            .as_ref()
                            .or_else(|| sparse_list.first())
                            .map(|sparse| sparse.channel.base_url.clone());
                        let kind = match url {
                            Some(url) => SubdirKind::Channel { url, platform },
                            None => SubdirKind::Custom,
                        };
                        let subdir = match matching {
                            Some(sparse) => Arc::new(SubdirState::Found(SubdirData::from_client(
                                LocalSubdirClient::new(sparse),
                            ))),
                            None => Arc::new(SubdirState::NotFound),
                        };
                        let b = barrier.clone();
                        let fut = box_future(async move {
                            b.set(subdir.clone()).expect("subdir was set twice");
                            Ok(PendingSubdirOk { subdir })
                        });
                        (kind, multi_channel, fut, barrier)
                    }
                };

                subdir_handles.push(SubdirHandle {
                    barrier,
                    kind,
                    data: RepoData {
                        multi_channel,
                        ..RepoData::default()
                    },
                    position: Some(position),
                });
                pending_subdirs.push(pending);
            }
        }

        Ok(Self {
            gateway,
            recursive,
            record_patch,
            reporter,
            platforms,
            detectors,
            direct_url_specs,
            direct_url_result,
            pending_pattern_specs,
            pattern_names_seen,
            seen,
            pending_package_specs,
            all_queued_specs: ahash::HashMap::default(),
            active_extras,
            fetched: ahash::HashMap::default(),
            subdir_handles,
            pending_subdirs,
            pending_records: FuturesUnordered::new(),
            discovery,
            notices,
        })
    }

    /// Spawn fetch futures for all direct URL specs (non-wasm).
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_direct_url_fetches(&mut self) -> Result<(), GatewayError> {
        for direct_url_spec in std::mem::take(&mut self.direct_url_specs) {
            let DirectUrlSpec { spec, url, name } = direct_url_spec;
            let gateway = self.gateway.clone();

            self.pending_records.push(box_future(async move {
                let query = super::direct_url_query::DirectUrlQuery::new(
                    url.clone(),
                    gateway.package_cache.clone(),
                    gateway.client.clone(),
                    spec.sha256,
                    spec.md5,
                )
                .with_concurrent_requests_semaphore(gateway.concurrent_requests_semaphore.clone());

                let records = query
                    .execute()
                    .await
                    .map_err(|e| GatewayError::DirectUrlQueryError(url.to_string(), e))?;

                // Check if record actually has the same name
                if let Some(record) = records.first()
                    && record.package_record.name != name
                {
                    return Err(GatewayError::UrlRecordNameMismatch(
                        record.package_record.name.as_source().to_string(),
                        name.as_source().to_string(),
                    ));
                }

                let (unique_base_deps, unique_extra_deps) =
                    super::subdir::extract_unique_deps_split(records.iter().map(|r| &**r));
                Ok((
                    AccumulateTarget::DirectUrl,
                    PendingRequest {
                        name: name.clone(),
                        specs: SourceSpecs::Input(vec![spec]),
                    },
                    PackageRecords {
                        records,
                        removed: Vec::new(),
                        unique_base_deps,
                        unique_extra_deps,
                    },
                ))
            }));
        }

        Ok(())
    }

    /// Spawn fetch futures for all direct URL specs (wasm - not supported).
    #[cfg(target_arch = "wasm32")]
    fn spawn_direct_url_fetches(&mut self) -> Result<(), GatewayError> {
        if let Some(spec) = self.direct_url_specs.first() {
            return Err(GatewayError::DirectUrlQueryNotSupported(
                spec.url.to_string(),
            ));
        }
        Ok(())
    }

    /// Drain `pending_package_specs` and spawn fetch futures for each.
    fn spawn_package_fetches(&mut self) {
        let pending_records = &mut self.pending_records;
        let reporter = &self.reporter;
        let subdir_handles = &self.subdir_handles;
        for (package_name, request) in self.pending_package_specs.drain() {
            for (idx, handle) in subdir_handles.iter().enumerate() {
                spawn_one_package_fetch(
                    pending_records,
                    package_name.clone(),
                    request.clone(),
                    AccumulateTarget::SubdirIndex(idx),
                    handle.barrier.clone(),
                    reporter.clone(),
                );
            }
            self.all_queued_specs.insert(package_name, request);
        }
    }

    /// Spawn fetches for every already-queued spec against a newly
    /// registered handle (used when CEP-42 introduces a subdir mid-query).
    fn spawn_package_fetches_for_new_handle(&mut self, handle_idx: usize) {
        let barrier = self.subdir_handles[handle_idx].barrier.clone();
        for (package_name, request) in &self.all_queued_specs {
            spawn_one_package_fetch(
                &mut self.pending_records,
                package_name.clone(),
                request.clone(),
                AccumulateTarget::SubdirIndex(handle_idx),
                barrier.clone(),
                self.reporter.clone(),
            );
        }
    }

    /// Extract dependencies from records and queue them if not seen.
    /// `queue_dependency` dedupes by name so re-walking the same deps on
    /// multi-subdir arrivals is harmless.
    fn queue_dependencies(&mut self, pkg: &PackageRecords, request: &PendingRequest) {
        let active: Vec<String> = self
            .active_extras
            .get(&request.name)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();

        match &request.specs {
            SourceSpecs::Transitive => {
                for dep in pkg.unique_base_deps.iter() {
                    self.queue_dependency(dep);
                }
                for extra in &active {
                    if let Some(deps) = pkg.unique_extra_deps.get(extra) {
                        for dep in deps.iter() {
                            self.queue_dependency(dep);
                        }
                    }
                }
            }
            SourceSpecs::Input(specs) => {
                for record in &pkg.records {
                    if !specs.iter().any(|s| s.matches(record.as_ref())) {
                        continue;
                    }
                    for dependency in &record.package_record.depends {
                        self.queue_dependency(dependency);
                    }
                    for extra in &active {
                        if let Some(deps) = record.package_record.extra_depends.get(extra) {
                            for dependency in deps {
                                self.queue_dependency(dependency);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Apply the query-local record patch and rebuild derived dependency data.
    fn patch_package_records(&self, mut pkg: PackageRecords) -> PackageRecords {
        let Some(patch) = &self.record_patch else {
            return pkg;
        };

        let mut changed = false;
        for record in &mut pkg.records {
            if let Some(patched) = patch(record.as_ref()) {
                *record = Arc::new(patched);
                changed = true;
            }
        }

        if changed {
            (pkg.unique_base_deps, pkg.unique_extra_deps) =
                extract_unique_deps_split(pkg.records.iter().map(AsRef::as_ref));
        }

        pkg
    }

    /// Walk the deps of newly-active extras against records that have
    /// already been fetched for `name`. Called when [`Self::queue_dependency`]
    /// activates one or more extras for a name whose records have already
    /// arrived. For Input-mode names, only deps from records matching the
    /// stored specs are walked.
    fn late_walk(&mut self, name: &PackageName, new_extras: &[String]) {
        // Collect deps so the borrow on `self.fetched` is released before
        // recursing into queue_dependency.
        let deps_to_walk: Vec<String> = {
            let Some(entry) = self.fetched.get(name) else {
                return;
            };
            match &entry.source {
                SourceSpecs::Transitive => entry
                    .pkgs
                    .iter()
                    .flat_map(|pkg| {
                        new_extras.iter().filter_map(move |ext| {
                            pkg.unique_extra_deps
                                .get(ext)
                                .map(|deps| deps.iter().cloned())
                        })
                    })
                    .flatten()
                    .collect(),
                SourceSpecs::Input(specs) => entry
                    .pkgs
                    .iter()
                    .flat_map(|pkg| {
                        pkg.records.iter().filter_map(move |record| {
                            if !specs.iter().any(|s| s.matches(record.as_ref())) {
                                return None;
                            }
                            Some(new_extras.iter().filter_map(move |ext| {
                                record
                                    .package_record
                                    .extra_depends
                                    .get(ext)
                                    .map(|deps| deps.iter().cloned())
                            }))
                        })
                    })
                    .flatten()
                    .flatten()
                    .collect(),
            }
        };

        for dep in &deps_to_walk {
            self.queue_dependency(dep);
        }
    }

    /// Queue a single dependency if not already seen. Allocates the name
    /// only when it is genuinely new (~500 unique names vs ~1M+ dependency
    /// strings on a large query).
    fn queue_dependency(&mut self, dependency: &str) {
        let (normalized, extras) = PackageName::name_and_extras_from_matchspec_str(dependency);
        let normalized_str: &str = &normalized;

        // Single hash lookup via EntryRef: either insert for a new name, or
        // observe and fall through to the merge-extras path for a known one.
        let is_new = match self.seen.entry_ref(normalized_str) {
            hashbrown::hash_map::EntryRef::Vacant(entry) => {
                entry.insert(());
                true
            }
            hashbrown::hash_map::EntryRef::Occupied(_) => false,
        };

        if is_new {
            let dependency_name = PackageName::from_matchspec_str_unchecked(dependency);
            if !extras.is_empty() {
                self.active_extras
                    .entry(dependency_name.clone())
                    .or_default()
                    .extend(extras);
            }
            self.pending_package_specs.insert(
                dependency_name.clone(),
                PendingRequest {
                    name: dependency_name,
                    specs: SourceSpecs::Transitive,
                },
            );
        } else if !extras.is_empty() {
            // Merge any extras the dep activates into the active set; if
            // records already arrived, walk the new extras against them.
            let dependency_name = PackageName::from_matchspec_str_unchecked(dependency);
            let newly_added: Vec<String> = {
                let existing = self
                    .active_extras
                    .entry(dependency_name.clone())
                    .or_default();
                extras
                    .into_iter()
                    .filter(|e| existing.insert(e.clone()))
                    .collect()
            };
            if !newly_added.is_empty() && self.fetched.contains_key(&dependency_name) {
                self.late_walk(&dependency_name, &newly_added);
            }
        }
    }

    /// Add matching records to the slot indicated by `target`. Removed
    /// packages are added unfiltered: they describe the fetched name, not a
    /// spec match.
    fn accumulate_records(
        &mut self,
        target: AccumulateTarget,
        pkg: PackageRecords,
        request: &PendingRequest,
    ) {
        let result = match target {
            AccumulateTarget::DirectUrl => self
                .direct_url_result
                .as_mut()
                .expect("direct-url fetch spawned without a direct-url bucket"),
            AccumulateTarget::SubdirIndex(idx) => &mut self.subdir_handles[idx].data,
        };

        let PackageRecords {
            records, removed, ..
        } = pkg;
        result.removed.extend(removed);

        match &request.specs {
            SourceSpecs::Transitive => {
                result.records.extend(records);
            }
            SourceSpecs::Input(specs) => {
                for record in &records {
                    if specs.iter().any(|s| s.matches(record.as_ref())) {
                        result.records.push(record.clone());
                    }
                }
            }
        }
    }

    /// Expand pattern specs based on the names provided by a resolved subdir.
    fn expand_pattern_specs_for_subdir(&mut self, subdir: &SubdirState) {
        if self.pending_pattern_specs.is_empty() {
            return;
        }

        let Some(names) = subdir.package_names() else {
            return;
        };

        for name_str in names {
            let Ok(name) = PackageName::try_from(name_str) else {
                continue;
            };
            if !self.pattern_names_seen.insert(name.clone()) {
                continue;
            }

            for (matcher, spec) in &self.pending_pattern_specs {
                if matcher.matches(&name) {
                    self.seen.insert(name.as_normalized().to_string(), ());
                    if let Some(extras) = spec.extras.as_ref() {
                        self.active_extras
                            .entry(name.clone())
                            .or_default()
                            .extend(extras.iter().cloned());
                    }
                    let pending = self
                        .pending_package_specs
                        .entry(name.clone())
                        .or_insert_with(|| PendingRequest {
                            name: name.clone(),
                            specs: SourceSpecs::Input(vec![]),
                        });
                    if let SourceSpecs::Input(input_specs) = &mut pending.specs {
                        input_specs.push(spec.clone());
                    }
                    break;
                }
            }
        }
    }

    /// Run the main event loop.
    async fn run(mut self) -> Result<RepoDataQueryOutput, GatewayError> {
        self.spawn_direct_url_fetches()?;

        loop {
            self.spawn_package_fetches();

            select_biased! {
                // Custom and local sources remain independent of channel discovery.
                subdir_result = self.pending_subdirs.select_next_some() => {
                    let ok = subdir_result?;
                    self.expand_pattern_specs_for_subdir(ok.subdir.as_ref());
                    if self.pending_subdirs.is_empty() && self.discovery.pending.is_empty() {
                        self.pending_pattern_specs.clear();
                        self.pattern_names_seen.clear();
                    }
                }

                // Drive channel discovery concurrently with package fetches.
                result = self.discovery.pending.select_next_some() => {
                    let fetched = result?;
                    if self.platforms.contains(&fetched.platform) {
                        self.expand_pattern_specs_for_subdir(fetched.subdir.as_ref());
                    }
                    for scheduled in self.discovery.observe(fetched)? {
                        self.schedule_transitive_subdir(scheduled);
                    }
                    if self.pending_subdirs.is_empty() && self.discovery.pending.is_empty() {
                        self.pending_pattern_specs.clear();
                        self.pattern_names_seen.clear();
                    }
                }

                // Handle any records that were fetched
                records = self.pending_records.select_next_some() => {
                    let (target, request, pkg) = records?;
                    let pkg = self.patch_package_records(pkg);

                    if self.recursive {
                        let entry =
                            self.fetched.entry(request.name.clone()).or_insert_with(|| {
                                FetchedEntry {
                                    pkgs: Vec::new(),
                                    source: request.specs.clone(),
                                }
                            });
                        entry.pkgs.push(pkg.clone());

                        self.queue_dependencies(&pkg, &request);
                    }

                    self.accumulate_records(target, pkg, &request);
                }

                // Handle any CEP-6 notices that were fetched
                batch = self.notices.pending.select_next_some() => {
                    self.notices.collect(self.reporter.as_deref(), batch);
                }

                // All futures have been handled, all subdirectories have been loaded and all
                // repodata records have been fetched
                complete => {
                    break;
                }
            }
        }

        self.finalize_channel_relations().await
    }

    /// Allocate a result slot for a transitively discovered (channel,
    /// platform) pair, spawn its subdir fetch, and kick off package
    /// fetches for every spec already queued.
    fn schedule_transitive_subdir(&mut self, scheduled: ScheduledSubdir) {
        let ScheduledSubdir {
            url,
            channel,
            platform,
            barrier,
        } = scheduled;
        if !self.platforms.contains(&platform) {
            return;
        }
        self.notices
            .queue(&self.gateway, &url, channel, self.reporter.clone());

        let handle_idx = self.subdir_handles.len();
        self.subdir_handles.push(SubdirHandle {
            barrier,
            kind: SubdirKind::Channel { url, platform },
            data: RepoData::default(),
            position: None,
        });
        self.spawn_package_fetches_for_new_handle(handle_idx);
    }

    /// Build the final [`RepoDataQueryOutput`]. When relations were
    /// observed, buckets sort by
    /// `(caller anchor, placement, priority, platform, original index)`:
    /// caller-supplied sources keep their positions, and discovered
    /// channels inherit the anchor of the user channel that introduced
    /// them and are placed before or after that whole caller source,
    /// depending on whether they outrank the introducing channel.
    /// Within a placement, CEP-42 priority orders the discovered channels.
    async fn finalize_channel_relations(mut self) -> Result<RepoDataQueryOutput, GatewayError> {
        let direct = self.direct_url_result;
        let mut handles = self.subdir_handles;
        let mut channel_order = None;

        if self.discovery.expander.enabled() && self.discovery.expander.has_observed_relations() {
            let mut resolution = self.discovery.expander.finalize()?;

            let priority_of: std::collections::HashMap<&ChannelUrl, usize> = resolution
                .order
                .iter()
                .enumerate()
                .map(|(i, u)| (u, i))
                .collect();
            let platform_idx_of: std::collections::HashMap<Subdir, usize> = self
                .discovery
                .expander
                .platforms()
                .iter()
                .copied()
                .enumerate()
                .map(|(i, p)| (p, i))
                .collect();

            // Caller-source position per user channel URL.
            let record_channel_position;
            let user_channel_position = if let Some(options) = &self.detectors {
                &options.channel_positions
            } else {
                record_channel_position = handles
                    .iter()
                    .filter_map(|h| match (&h.kind, h.position) {
                        (SubdirKind::Channel { url, .. }, Some(position)) => {
                            Some((url.clone(), position))
                        }
                        _ => None,
                    })
                    .collect::<HashMap<_, _>>();
                &record_channel_position
            };

            // Anchors derive from the final edge set, independent of
            // fetch completion order.
            let mut users_by_position: Vec<(SourcePosition, ChannelUrl)> = user_channel_position
                .iter()
                .map(|(url, position)| (*position, url.clone()))
                .collect();
            users_by_position.sort();
            let user_priority: Vec<ChannelUrl> =
                users_by_position.into_iter().map(|(_, url)| url).collect();
            let anchor_of = self.discovery.expander.anchors(&user_priority);

            let mut tagged: Vec<(BucketOrder, SubdirHandle)> = handles
                .into_iter()
                .enumerate()
                .map(|(original_index, h)| {
                    let (anchor, placement, priority, platform) = match (&h.kind, h.position) {
                        (SubdirKind::Custom, Some(position)) => {
                            (position.source, Placement::Source, position.member, 0_usize)
                        }
                        (SubdirKind::Channel { platform, .. }, Some(position)) => {
                            let p = platform_idx_of.get(platform).copied().unwrap_or(usize::MAX);
                            (position.source, Placement::Source, position.member, p)
                        }
                        (SubdirKind::Channel { url, platform }, None) => {
                            let p = platform_idx_of.get(platform).copied().unwrap_or(usize::MAX);
                            let (anchor, placement, priority) = channel_placement(
                                url,
                                user_channel_position,
                                &priority_of,
                                &anchor_of,
                            );
                            (anchor, placement, priority, p)
                        }
                        (SubdirKind::Custom, None) => {
                            unreachable!("custom sources are always caller-supplied")
                        }
                    };
                    let order = BucketOrder {
                        anchor,
                        placement,
                        priority,
                        platform,
                        original_index,
                    };
                    (order, h)
                })
                .collect();
            tagged.sort_by_key(|(order, _)| *order);
            handles = tagged.into_iter().map(|(_, h)| h).collect();
            if self.detectors.is_some() {
                let detector_order: HashMap<_, _> = resolution
                    .order
                    .iter()
                    .filter_map(|url| {
                        let stable_url = user_channel_position
                            .get_key_value(url)
                            .map(|(url, _)| url)
                            .or_else(|| anchor_of.get_key_value(url).map(|(url, _)| url))?;
                        Some((
                            stable_url,
                            channel_placement(url, user_channel_position, &priority_of, &anchor_of),
                        ))
                    })
                    .collect();
                resolution.order.sort_by_key(|url| {
                    detector_order.get(url).copied().unwrap_or((
                        usize::MAX,
                        Placement::AfterSource,
                        usize::MAX,
                    ))
                });
            }
            channel_order = Some(resolution.order);
        }

        let mut expansion = self.discovery.finish(channel_order)?;
        let mut repodata: Vec<RepoData> =
            Vec::with_capacity(handles.len() + usize::from(direct.is_some()));
        if let Some(d) = direct {
            repodata.push(d);
        }
        repodata.extend(handles.into_iter().map(|h| h.data));
        let mut warnings: Vec<_> = std::mem::take(&mut expansion.warnings)
            .into_iter()
            .map(GatewayWarning::from)
            .collect();
        let virtual_package_detectors = if let Some(mut options) = self.detectors {
            options.wanted_names.extend(referenced_virtual_packages(
                repodata
                    .iter()
                    .flat_map(|data| data.iter())
                    .map(|record| &record.package_record),
            ));
            let query = VirtualPackageDetectorsQuery::new(
                self.gateway,
                Vec::new(),
                vec![options.target_platform, Subdir::NoArch],
                self.reporter,
            )
            .channel_relations(options.channel_relations_mode)
            .channel_relations_max_depth(options.channel_relations_max_depth);
            let discovery = query.collect_from_expansion(&expansion).await?;
            warnings.extend(discovery.warnings);
            Some(QueryVirtualPackageDetectors {
                target_platform: options.target_platform,
                wanted_names: options.wanted_names,
                registrations: discovery.registrations,
                rejected: discovery.rejected,
            })
        } else {
            None
        };
        Ok(RepoDataQueryOutput {
            repodata,
            notices: self.notices.collected,
            warnings,
            virtual_package_detectors,
        })
    }
}

/// How a channel subdir fetch should handle errors from
/// `get_or_create_subdir`.
#[derive(Clone, Copy)]
pub(super) enum FetchErrorPolicy {
    /// Surface the error to the caller (user-supplied channels).
    Propagate,
    /// Emit a [`ChannelRelationsWarning::DiscoveryFetchFailed`] and
    /// treat the subdir as empty.
    SwallowAsWarning,
    /// Wrap in [`GatewayError::ChannelRelationsError`] (Strict mode for
    /// transitively discovered channels).
    WrapAsChannelRelationsError,
}

/// Translate a subdir fetch error into the policy-prescribed outcome.
/// Returns `Ok((SubdirState::NotFound, Some(warning)))` for
/// `SwallowAsWarning` so callers can proceed as if the subdir were
/// absent; returns `Err` for `Propagate` or
/// `WrapAsChannelRelationsError`.
pub(super) fn apply_fetch_error_policy(
    err: GatewayError,
    url: &ChannelUrl,
    platform: Subdir,
    policy: FetchErrorPolicy,
) -> Result<(Arc<SubdirState>, Option<ChannelRelationsWarning>), GatewayError> {
    // A channel publishing only some platforms is valid; treat a
    // missing subdir as empty. The subdir builder already does this
    // for every platform except noarch.
    if !matches!(policy, FetchErrorPolicy::Propagate)
        && matches!(err, GatewayError::SubdirNotFoundError(_))
    {
        return Ok((Arc::new(SubdirState::NotFound), None));
    }
    match policy {
        FetchErrorPolicy::Propagate => Err(err),
        FetchErrorPolicy::WrapAsChannelRelationsError | FetchErrorPolicy::SwallowAsWarning => {
            let warning = ChannelRelationsWarning::DiscoveryFetchFailed {
                url: url.clone(),
                platform,
                error: err.to_string(),
            };
            if matches!(policy, FetchErrorPolicy::WrapAsChannelRelationsError) {
                Err(GatewayError::ChannelRelationsError(warning.to_string()))
            } else {
                Ok((Arc::new(SubdirState::NotFound), Some(warning)))
            }
        }
    }
}

/// Outcome of a custom or local subdir fetch.
struct PendingSubdirOk {
    subdir: Arc<SubdirState>,
}

/// Push a future onto `pending_records` that awaits the subdir's
/// barrier, fetches records for `package_name`, and tags the outcome
/// with `target`.
fn spawn_one_package_fetch(
    pending_records: &mut FuturesUnordered<BoxFuture<PendingRecordsResult>>,
    package_name: PackageName,
    request: PendingRequest,
    target: AccumulateTarget,
    barrier: Arc<BarrierCell<Arc<SubdirState>>>,
    reporter: Option<Arc<dyn Reporter>>,
) {
    pending_records.push(box_future(async move {
        let subdir = barrier.wait().await;
        match subdir.as_ref() {
            SubdirState::Found(subdir) => subdir
                .get_or_fetch_package_records(&package_name, reporter)
                .await
                .map(|pkg| (target, request, pkg)),
            SubdirState::NotFound => Ok((target, request, PackageRecords::default())),
        }
    }));
}

/// Result type for pending record fetches.
type PendingSubdirResult = Result<PendingSubdirOk, GatewayError>;
type PendingRecordsResult =
    Result<(AccumulateTarget, PendingRequest, PackageRecords), GatewayError>;

impl IntoFuture for RepoDataQuery {
    type Output = Result<RepoDataQueryOutput, GatewayError>;
    type IntoFuture = BoxFuture<Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        box_future(self.execute())
    }
}

/// Represents a query for package names to execute with a [`Gateway`](super::Gateway).
///
/// When executed the query will asynchronously load the package names from all
/// subdirectories (combination of channels and platforms).
#[derive(Clone)]
pub struct NamesQuery {
    /// The gateway that manages all resources
    gateway: Arc<GatewayInner>,

    /// The channels to fetch from
    channels: Vec<Channel>,

    /// The platforms the fetch from
    platforms: Vec<Subdir>,

    /// The reporter to use by the query.
    reporter: Option<Arc<dyn Reporter>>,

    /// Whether to fetch CEP-6 notices for this query.
    channel_notices: bool,

    /// CEP-42 channel relations handling mode.
    channel_relations_mode: ChannelRelationsMode,

    /// Maximum recursion depth when following CEP-42 `channel_relations`.
    channel_relations_max_depth: usize,
}

impl NamesQuery {
    /// Constructs a new instance. This should not be called directly, use
    /// [`Gateway::names`] instead.
    pub(super) fn new(
        gateway: Arc<GatewayInner>,
        channels: Vec<Channel>,
        platforms: Vec<Subdir>,
    ) -> Self {
        Self {
            gateway,
            channels,
            platforms,

            reporter: None,
            channel_notices: false,
            channel_relations_mode: ChannelRelationsMode::default(),
            channel_relations_max_depth: DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH,
        }
    }

    /// Enable or disable fetching CEP-6 channel notices. Disabled by default.
    #[must_use]
    pub fn channel_notices(self, enabled: bool) -> Self {
        Self {
            channel_notices: enabled,
            ..self
        }
    }

    /// Sets the reporter to use for this query.
    ///
    /// The reporter is notified of important evens during the execution of the
    /// query. This allows reporting progress back to a user.
    pub fn with_reporter(self, reporter: impl Reporter + 'static) -> Self {
        Self {
            reporter: Some(Arc::new(reporter)),
            ..self
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
    /// No effect when the mode is [`ChannelRelationsMode::Disabled`].
    #[must_use]
    pub fn channel_relations_max_depth(self, depth: usize) -> Self {
        Self {
            channel_relations_max_depth: depth,
            ..self
        }
    }

    /// Execute the query and return the package names along with any
    /// non-fatal CEP-42 warnings.
    pub async fn execute(self) -> Result<NamesQueryOutput, GatewayError> {
        let mut discovery = ChannelDiscovery::new(
            self.gateway.clone(),
            self.platforms.clone(),
            self.channel_relations_mode,
            self.channel_relations_max_depth,
            self.reporter.clone(),
            None,
            false,
        );
        let mut notices = NoticeCollector::new(self.channel_notices);

        for channel in self.channels {
            let (url, channel) = discovery.register_user_channel(channel);
            notices.queue(&self.gateway, &url, channel.clone(), self.reporter.clone());
            for &platform in &self.platforms {
                discovery.schedule_user_subdir(url.clone(), channel.clone(), platform);
            }
        }

        let mut names: std::collections::HashSet<String> = std::collections::HashSet::default();

        loop {
            select_biased! {
                result = discovery.pending.select_next_some() => {
                    let fetched = result?;
                    if let Some(subdir_names) = fetched.subdir.package_names() {
                        names.extend(subdir_names);
                    }
                    for scheduled in discovery.observe(fetched)? {
                        notices.queue(&self.gateway, &scheduled.url, scheduled.channel, self.reporter.clone());
                    }
                }

                batch = notices.pending.select_next_some() => {
                    notices.collect(self.reporter.as_deref(), batch);
                }

                complete => {
                    break;
                }
            }
        }

        if discovery.expander.enabled() && discovery.expander.has_observed_relations() {
            // Names are unordered; finalize for depth/cycle diagnostics.
            discovery.expander.finalize()?;
        }

        let names = names
            .into_iter()
            .map(PackageName::try_from)
            .collect::<Result<Vec<PackageName>, _>>()?;
        Ok(NamesQueryOutput {
            names,
            notices: notices.collected,
            warnings: discovery
                .expander
                .take_warnings()
                .into_iter()
                .map(GatewayWarning::from)
                .collect(),
        })
    }
}

impl IntoFuture for NamesQuery {
    type Output = Result<NamesQueryOutput, GatewayError>;
    type IntoFuture = BoxFuture<Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        box_future(self.execute())
    }
}
