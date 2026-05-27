use std::{
    collections::HashSet,
    future::{Future, IntoFuture},
    sync::Arc,
};

use futures::{FutureExt, StreamExt, select_biased, stream::FuturesUnordered};
use itertools::Itertools;
use rattler_conda_types::{
    Channel, ChannelUrl, MatchSpec, Matches, PackageName, PackageNameMatcher, Platform,
    RepoDataRecord,
};
use url::Url;

use super::{
    BarrierCell, GatewayError, GatewayInner, RepoData,
    channel_expander::{ChannelExpander, ChannelRelationsMode},
    channel_relations::DEFAULT_MAX_DEPTH,
    source::{CustomSourceClient, Source},
    subdir::{PackageRecords, Subdir, SubdirData},
};
use crate::Reporter;

/// Represents a query to execute with a [`Gateway`].
///
/// When executed the query will asynchronously load the repodata from all
/// subdirectories (combination of sources and platforms).
///
/// Most processing will happen on the background so downloading and parsing
/// can happen simultaneously.
///
/// Repodata is cached by the [`Gateway`] so executing the same query twice
/// with the same sources will not result in the repodata being fetched
/// twice.
#[derive(Clone)]
pub struct RepoDataQuery {
    /// The gateway that manages all resources
    gateway: Arc<GatewayInner>,

    /// The sources to fetch from (channels or custom sources)
    sources: Vec<Source>,

    /// The platforms the fetch from
    platforms: Vec<Platform>,

    /// The specs to fetch records for
    specs: Vec<MatchSpec>,

    /// Whether to recursively fetch dependencies
    recursive: bool,

    /// The reporter to use by the query.
    reporter: Option<Arc<dyn Reporter>>,

    /// CEP-42 channel relations handling mode.
    channel_relations_mode: ChannelRelationsMode,

    /// Maximum recursion depth when following CEP-42 `channel_relations`.
    channel_relations_max_depth: usize,
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
    barrier: Arc<BarrierCell<Arc<Subdir>>>,
    kind: SubdirKind,
    data: RepoData,
}

/// Origin of a [`SubdirHandle`]; drives final-result reordering.
#[derive(Clone)]
enum SubdirKind {
    /// Channel subdirectory; `url` is the canonical base URL used as
    /// the CEP-42 resolver's identifier.
    Channel { url: ChannelUrl, platform: Platform },
    /// Custom source; not subject to CEP-42 ordering.
    Custom,
}

/// Where a fetched batch of records should land.
#[derive(Clone, Copy, Debug)]
enum AccumulateTarget {
    // Only constructed by `spawn_direct_url_fetches` which is non-wasm.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    DirectUrl,
    Subdir(usize),
}

impl RepoDataQuery {
    /// Constructs a new instance. This should not be called directly, use
    /// [`Gateway::query`] instead.
    pub(super) fn new(
        gateway: Arc<GatewayInner>,
        sources: Vec<Source>,
        platforms: Vec<Platform>,
        specs: Vec<MatchSpec>,
    ) -> Self {
        Self {
            gateway,
            sources,
            platforms,
            specs,

            recursive: false,
            reporter: None,
            channel_relations_mode: ChannelRelationsMode::default(),
            channel_relations_max_depth: DEFAULT_MAX_DEPTH,
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
    /// [`DEFAULT_MAX_DEPTH`](super::channel_relations::DEFAULT_MAX_DEPTH).
    /// No effect when the mode is [`ChannelRelationsMode::Disabled`].
    #[must_use]
    pub fn channel_relations_max_depth(self, depth: usize) -> Self {
        Self {
            channel_relations_max_depth: depth,
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

    /// Execute the query and return the resulting repodata records.
    pub async fn execute(self) -> Result<Vec<RepoData>, GatewayError> {
        // Short circuit if there are no specs
        if self.specs.is_empty() {
            return Ok(Vec::default());
        }

        let executor = QueryExecutor::new(self)?;
        executor.run().await
    }
}

/// Owns all mutable state during query execution and provides methods for each phase.
struct QueryExecutor {
    // Configuration (immutable after construction)
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    gateway: Arc<GatewayInner>,
    recursive: bool,
    reporter: Option<Arc<dyn Reporter>>,

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

    /// CEP-42 expansion state.
    expander: ChannelExpander,
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
            reporter,
            channel_relations_mode,
            channel_relations_max_depth,
        } = query;

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

        let mut expander = ChannelExpander::new(
            channel_relations_mode,
            channel_relations_max_depth,
            platforms.clone(),
        );

        let sources_and_platforms = sources
            .into_iter()
            .cartesian_product(platforms)
            .collect_vec();

        let mut subdir_handles = Vec::with_capacity(sources_and_platforms.len());
        let pending_subdirs = FuturesUnordered::new();

        for (source, platform) in sources_and_platforms {
            let barrier = Arc::new(BarrierCell::new());

            let (kind, pending) = match source {
                Source::Channel(channel) => {
                    let (url, channel) = expander.register_user_channel(channel);
                    let kind = SubdirKind::Channel {
                        url: url.clone(),
                        platform,
                    };
                    let fut = build_channel_subdir_future(
                        gateway.clone(),
                        channel,
                        platform,
                        url,
                        reporter.clone(),
                        barrier.clone(),
                        FetchErrorPolicy::Propagate,
                    );
                    (kind, fut)
                }
                Source::Custom(custom_source) => {
                    let client = CustomSourceClient::new(custom_source, platform);
                    let subdir = Arc::new(Subdir::Found(SubdirData::from_client(client)));
                    let b = barrier.clone();
                    let fut = box_future(async move {
                        b.set(subdir.clone()).expect("subdir was set twice");
                        Ok(PendingSubdirOk {
                            subdir,
                            kind_url_and_platform: None,
                        })
                    });
                    (SubdirKind::Custom, fut)
                }
            };

            subdir_handles.push(SubdirHandle {
                barrier,
                kind,
                data: RepoData::default(),
            });
            pending_subdirs.push(pending);
        }

        Ok(Self {
            gateway,
            recursive,
            reporter,
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
            expander,
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
                    AccumulateTarget::Subdir(idx),
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
                AccumulateTarget::Subdir(handle_idx),
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

    /// Add matching records to the slot indicated by `target`.
    fn accumulate_records(
        &mut self,
        target: AccumulateTarget,
        records: Vec<Arc<RepoDataRecord>>,
        request: &PendingRequest,
    ) {
        let result = match target {
            AccumulateTarget::DirectUrl => self
                .direct_url_result
                .as_mut()
                .expect("direct-url fetch spawned without a direct-url bucket"),
            AccumulateTarget::Subdir(idx) => &mut self.subdir_handles[idx].data,
        };

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
    fn expand_pattern_specs_for_subdir(&mut self, subdir: &Subdir) {
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
    async fn run(mut self) -> Result<Vec<RepoData>, GatewayError> {
        self.spawn_direct_url_fetches()?;

        loop {
            self.spawn_package_fetches();

            select_biased! {
                // Handle any error that was emitted by the pending subdirs
                subdir_result = self.pending_subdirs.select_next_some() => {
                    let ok = subdir_result?;
                    let PendingSubdirOk { subdir, kind_url_and_platform } = ok;
                    self.expand_pattern_specs_for_subdir(subdir.as_ref());
                    if let Some((url, platform)) = kind_url_and_platform {
                        self.expand_relations_for_subdir(&url, platform, subdir.as_ref())?;
                    }
                    if self.pending_subdirs.is_empty() {
                        self.pending_pattern_specs.clear();
                        self.pattern_names_seen.clear();
                    }
                }

                // Handle any records that were fetched
                records = self.pending_records.select_next_some() => {
                    let (target, request, pkg) = records?;

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

                    self.accumulate_records(target, pkg.records, &request);
                }

                // All futures have been handled, all subdirectories have been loaded and all
                // repodata records have been fetched
                complete => {
                    break;
                }
            }
        }

        self.finalize_channel_relations()
    }

    /// Hand a freshly resolved subdir to the expander; schedule fetches
    /// for any newly discovered (channel, platform) pairs. In `Strict`
    /// mode propagates an incremental cycle/parse error so the
    /// executor aborts the remaining in-flight fetches.
    fn expand_relations_for_subdir(
        &mut self,
        channel_url: &ChannelUrl,
        platform: Platform,
        subdir: &Subdir,
    ) -> Result<(), GatewayError> {
        let new_pairs = self.expander.observe(channel_url, platform, subdir)?;
        for (url, channel, plat) in new_pairs {
            self.schedule_transitive_subdir(url, channel, plat);
        }
        Ok(())
    }

    /// Allocate a result slot for a transitively discovered (channel,
    /// platform) pair, spawn its subdir fetch, and kick off package
    /// fetches for every spec already queued.
    fn schedule_transitive_subdir(
        &mut self,
        url: ChannelUrl,
        channel: Arc<Channel>,
        platform: Platform,
    ) {
        let barrier = Arc::new(BarrierCell::new());

        let policy = if self.expander.strict() {
            FetchErrorPolicy::WrapAsChannelRelationsError
        } else {
            FetchErrorPolicy::SwallowAndWarn
        };
        let fut = build_channel_subdir_future(
            self.gateway.clone(),
            channel,
            platform,
            url.clone(),
            self.reporter.clone(),
            barrier.clone(),
            policy,
        );
        self.pending_subdirs.push(fut);

        let handle_idx = self.subdir_handles.len();
        self.subdir_handles.push(SubdirHandle {
            barrier,
            kind: SubdirKind::Channel { url, platform },
            data: RepoData::default(),
        });
        self.spawn_package_fetches_for_new_handle(handle_idx);
    }

    /// Build the final `Vec<RepoData>`. When CEP-42 is enabled AND at
    /// least one subdir contributed relations, reorder channel entries
    /// by the resolved priority; otherwise preserve the original
    /// construction order so mixed channel/custom queries with no
    /// declared relations are not silently re-tiered.
    fn finalize_channel_relations(self) -> Result<Vec<RepoData>, GatewayError> {
        let direct = self.direct_url_result;
        let mut handles = self.subdir_handles;

        if self.expander.enabled() && self.expander.has_observed_relations() {
            let resolution = self.expander.finalize();

            if let Some(msg) = self.expander.strict_error(&resolution) {
                return Err(GatewayError::ChannelRelationsError(msg));
            }

            // Layout: direct-url bucket at 0, channels by CEP-42 priority
            // (platform list tiebreaks), custom sources last in construction
            // order.
            let priority_of: std::collections::HashMap<&ChannelUrl, usize> = resolution
                .order
                .iter()
                .enumerate()
                .map(|(i, u)| (u, i))
                .collect();
            let platform_idx_of: std::collections::HashMap<Platform, usize> = self
                .expander
                .platforms()
                .iter()
                .copied()
                .enumerate()
                .map(|(i, p)| (p, i))
                .collect();

            let mut tagged: Vec<(usize, SubdirHandle)> = handles.into_iter().enumerate().collect();
            tagged.sort_by_key(|(orig_idx, h)| match &h.kind {
                SubdirKind::Channel { url, platform } => {
                    let prio = priority_of.get(url).copied().unwrap_or(usize::MAX);
                    let plat = platform_idx_of.get(platform).copied().unwrap_or(usize::MAX);
                    (1_usize, prio, plat, *orig_idx)
                }
                SubdirKind::Custom => (2, 0, 0, *orig_idx),
            });
            handles = tagged.into_iter().map(|(_, h)| h).collect();
        }

        let mut final_result: Vec<RepoData> =
            Vec::with_capacity(handles.len() + usize::from(direct.is_some()));
        if let Some(d) = direct {
            final_result.push(d);
        }
        final_result.extend(handles.into_iter().map(|h| h.data));
        Ok(final_result)
    }
}

/// How a channel subdir fetch should handle errors from
/// `get_or_create_subdir`.
#[derive(Clone, Copy)]
enum FetchErrorPolicy {
    /// Surface the error to the caller (user-supplied channels).
    Propagate,
    /// Log via `tracing::warn!` and treat the subdir as empty.
    SwallowAndWarn,
    /// Wrap in [`GatewayError::ChannelRelationsError`] (Strict mode for
    /// transitively discovered channels).
    WrapAsChannelRelationsError,
}

/// Build a future that fetches a channel subdir, sets the barrier, and
/// applies `policy` to any fetch error. Used by `RepoDataQuery`'s
/// executor; `NamesQuery` uses the simpler [`spawn_names_fetch`]
/// wrapper around the same [`fetch_subdir_with_policy`] core.
fn build_channel_subdir_future(
    gateway: Arc<GatewayInner>,
    channel: Arc<Channel>,
    platform: Platform,
    url: ChannelUrl,
    reporter: Option<Arc<dyn Reporter>>,
    barrier: Arc<BarrierCell<Arc<Subdir>>>,
    policy: FetchErrorPolicy,
) -> BoxFuture<PendingSubdirResult> {
    box_future(async move {
        let subdir =
            fetch_subdir_with_policy(&gateway, &channel, platform, &url, reporter, policy).await?;
        barrier.set(subdir.clone()).expect("subdir was set twice");
        Ok(PendingSubdirOk {
            subdir,
            kind_url_and_platform: Some((url, platform)),
        })
    })
}

/// Fetch a channel subdir and apply `policy` to any error. Shared core
/// for the channel-fetch futures spawned by both `RepoDataQuery` and
/// `NamesQuery`.
async fn fetch_subdir_with_policy(
    gateway: &GatewayInner,
    channel: &Channel,
    platform: Platform,
    url: &ChannelUrl,
    reporter: Option<Arc<dyn Reporter>>,
    policy: FetchErrorPolicy,
) -> Result<Arc<Subdir>, GatewayError> {
    match gateway
        .get_or_create_subdir(channel, platform, reporter)
        .await
    {
        Ok(subdir) => Ok(subdir),
        Err(err) => apply_fetch_error_policy(err, url, platform, policy),
    }
}

/// Translate a subdir fetch error into the policy-prescribed outcome.
/// Returns `Ok(Subdir::NotFound)` for `SwallowAndWarn` so callers can
/// proceed as if the subdir were absent; returns `Err` for `Propagate`
/// or `WrapAsChannelRelationsError`.
fn apply_fetch_error_policy(
    err: GatewayError,
    url: &ChannelUrl,
    platform: Platform,
    policy: FetchErrorPolicy,
) -> Result<Arc<Subdir>, GatewayError> {
    match policy {
        FetchErrorPolicy::Propagate => Err(err),
        FetchErrorPolicy::WrapAsChannelRelationsError => {
            Err(GatewayError::ChannelRelationsError(format!(
                "failed to fetch transitively discovered channel \
                 `{url}` for platform `{platform}`: {err}"
            )))
        }
        FetchErrorPolicy::SwallowAndWarn => {
            tracing::warn!(
                "failed to fetch transitively discovered channel \
                 `{url}` for platform `{platform}`: {err}. \
                 treating the subdir as empty."
            );
            Ok(Arc::new(Subdir::NotFound))
        }
    }
}

/// Outcome of a pending subdir fetch. The key is `Some` for channel
/// sources (used to register CEP-42 relations) and `None` for custom
/// sources.
struct PendingSubdirOk {
    subdir: Arc<Subdir>,
    kind_url_and_platform: Option<(ChannelUrl, Platform)>,
}

/// Push a future onto `pending_records` that awaits the subdir's
/// barrier, fetches records for `package_name`, and tags the outcome
/// with `target`.
fn spawn_one_package_fetch(
    pending_records: &mut FuturesUnordered<BoxFuture<PendingRecordsResult>>,
    package_name: PackageName,
    request: PendingRequest,
    target: AccumulateTarget,
    barrier: Arc<BarrierCell<Arc<Subdir>>>,
    reporter: Option<Arc<dyn Reporter>>,
) {
    pending_records.push(box_future(async move {
        let subdir = barrier.wait().await;
        match subdir.as_ref() {
            Subdir::Found(subdir) => subdir
                .get_or_fetch_package_records(&package_name, reporter)
                .await
                .map(|pkg| (target, request, pkg)),
            Subdir::NotFound => Ok((target, request, PackageRecords::default())),
        }
    }));
}

#[cfg(target_arch = "wasm32")]
type BoxFuture<T> = futures::future::LocalBoxFuture<'static, T>;

#[cfg(target_arch = "wasm32")]
fn box_future<T, F: Future<Output = T> + 'static>(future: F) -> BoxFuture<T> {
    future.boxed_local()
}

#[cfg(not(target_arch = "wasm32"))]
type BoxFuture<T> = futures::future::BoxFuture<'static, T>;

#[cfg(not(target_arch = "wasm32"))]
fn box_future<T, F: Future<Output = T> + Send + 'static>(future: F) -> BoxFuture<T> {
    future.boxed()
}

/// Result type for pending record fetches.
type PendingSubdirResult = Result<PendingSubdirOk, GatewayError>;
type PendingRecordsResult =
    Result<(AccumulateTarget, PendingRequest, PackageRecords), GatewayError>;

impl IntoFuture for RepoDataQuery {
    type Output = Result<Vec<RepoData>, GatewayError>;
    type IntoFuture = BoxFuture<Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        box_future(self.execute())
    }
}

/// Represents a query for package names to execute with a [`Gateway`].
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
    platforms: Vec<Platform>,

    /// The reporter to use by the query.
    reporter: Option<Arc<dyn Reporter>>,

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
        platforms: Vec<Platform>,
    ) -> Self {
        Self {
            gateway,
            channels,
            platforms,

            reporter: None,
            channel_relations_mode: ChannelRelationsMode::default(),
            channel_relations_max_depth: DEFAULT_MAX_DEPTH,
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
    /// [`DEFAULT_MAX_DEPTH`](super::channel_relations::DEFAULT_MAX_DEPTH).
    /// No effect when the mode is [`ChannelRelationsMode::Disabled`].
    #[must_use]
    pub fn channel_relations_max_depth(self, depth: usize) -> Self {
        Self {
            channel_relations_max_depth: depth,
            ..self
        }
    }

    /// Execute the query and return the package names.
    pub async fn execute(self) -> Result<Vec<PackageName>, GatewayError> {
        let mut expander = ChannelExpander::new(
            self.channel_relations_mode,
            self.channel_relations_max_depth,
            self.platforms.clone(),
        );

        let mut pending: FuturesUnordered<BoxFuture<NamesFetchResult>> = FuturesUnordered::new();
        for channel in self.channels {
            let (url, channel_arc) = expander.register_user_channel(channel);
            for &platform in &self.platforms {
                pending.push(spawn_names_fetch(
                    self.gateway.clone(),
                    channel_arc.clone(),
                    platform,
                    url.clone(),
                    self.reporter.clone(),
                    FetchErrorPolicy::Propagate,
                ));
            }
        }

        let mut names: std::collections::HashSet<String> = std::collections::HashSet::default();
        let strict = expander.strict();
        let policy = if strict {
            FetchErrorPolicy::WrapAsChannelRelationsError
        } else {
            FetchErrorPolicy::SwallowAndWarn
        };

        while let Some(result) = pending.next().await {
            let (url, platform, subdir) = result?;
            if let Some(subdir_names) = subdir.package_names() {
                names.extend(subdir_names);
            }
            for (new_url, new_channel, new_plat) in expander.observe(&url, platform, &subdir)? {
                pending.push(spawn_names_fetch(
                    self.gateway.clone(),
                    new_channel,
                    new_plat,
                    new_url,
                    self.reporter.clone(),
                    policy,
                ));
            }
        }

        if expander.enabled() && expander.has_observed_relations() {
            let resolution = expander.finalize();
            if let Some(msg) = expander.strict_error(&resolution) {
                return Err(GatewayError::ChannelRelationsError(msg));
            }
        }

        Ok(names
            .into_iter()
            .map(PackageName::try_from)
            .collect::<Result<Vec<PackageName>, _>>()?)
    }
}

type NamesFetchResult = Result<(ChannelUrl, Platform, Arc<Subdir>), GatewayError>;

/// Build a future that fetches a channel subdir for `NamesQuery` and
/// applies `policy` to any fetch error.
fn spawn_names_fetch(
    gateway: Arc<GatewayInner>,
    channel: Arc<Channel>,
    platform: Platform,
    url: ChannelUrl,
    reporter: Option<Arc<dyn Reporter>>,
    policy: FetchErrorPolicy,
) -> BoxFuture<NamesFetchResult> {
    box_future(async move {
        let subdir =
            fetch_subdir_with_policy(&gateway, &channel, platform, &url, reporter, policy).await?;
        Ok((url, platform, subdir))
    })
}

impl IntoFuture for NamesQuery {
    type Output = Result<Vec<PackageName>, GatewayError>;
    type IntoFuture = BoxFuture<Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        box_future(self.execute())
    }
}
