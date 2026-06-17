use std::{
    collections::HashSet,
    future::{Future, IntoFuture},
    sync::Arc,
};

use futures::{FutureExt, StreamExt, select_biased, stream::FuturesUnordered};
use itertools::Itertools;
use rattler_conda_types::{
    Channel, MatchSpec, Matches, PackageName, PackageNameMatcher, Platform, RepoDataRecord,
};
use url::Url;

use super::{
    BarrierCell, GatewayError, GatewayInner, RepoData,
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

/// Handle to a pending subdirectory.
struct SubdirHandle {
    result_index: usize,
    barrier: Arc<BarrierCell<Arc<Subdir>>>,
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

    /// Specs with glob/regex patterns that need expansion
    pending_pattern_specs: Vec<(PackageNameMatcher, MatchSpec)>,
    /// Track names already considered for pattern expansion (across subdirs)
    pattern_names_seen: HashSet<PackageName>,

    // Mutable state during execution
    /// Normalized (lowercase) package names we've already queued.
    seen: hashbrown::HashMap<String, (), ahash::RandomState>,
    pending_package_specs: ahash::HashMap<PackageName, PendingRequest>,
    /// Per-name set of extras that are currently active. Grows monotonically
    /// as new extras are discovered via top-level specs and dep parsing.
    active_extras: ahash::HashMap<PackageName, ahash::HashSet<String>>,
    /// Records cached by name across subdirs. Used to re-walk a name's
    /// records when an extra activates after the first arrival.
    fetched: ahash::HashMap<PackageName, FetchedEntry>,

    // Subdir management
    subdir_handles: Vec<SubdirHandle>,
    pending_subdirs: FuturesUnordered<BoxFuture<PendingSubdirResult>>,

    // Record fetching
    pending_records: FuturesUnordered<BoxFuture<PendingRecordsResult>>,

    // Results
    result: Vec<RepoData>,
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

        // Result offset for direct url queries
        let direct_url_offset = usize::from(!direct_url_specs.is_empty());

        // Expand sources into (source, platform) pairs for each platform
        // For channels: use gateway's get_or_create_subdir
        // For custom sources: create CustomSourceClient adapters
        let sources_and_platforms = sources
            .into_iter()
            .cartesian_product(platforms)
            .collect_vec();

        // Create barrier cells for each subdirectory
        let mut subdir_handles = Vec::with_capacity(sources_and_platforms.len());
        let pending_subdirs = FuturesUnordered::new();

        for (subdir_idx, (source, platform)) in sources_and_platforms.into_iter().enumerate() {
            let barrier = Arc::new(BarrierCell::new());
            subdir_handles.push(SubdirHandle {
                result_index: subdir_idx + direct_url_offset,
                barrier: barrier.clone(),
            });

            let pending = match source {
                Source::Channel(channel) => {
                    let inner = gateway.clone();
                    let reporter = reporter.clone();
                    box_future(async move {
                        let subdir = inner
                            .get_or_create_subdir(&channel, platform, reporter)
                            .await?;
                        barrier.set(subdir.clone()).expect("subdir was set twice");
                        Ok(subdir)
                    })
                }
                Source::Custom(custom_source) => {
                    // For custom sources, create an adapter that wraps the source
                    // for the specific platform.
                    let client = CustomSourceClient::new(custom_source, platform);
                    let subdir = Arc::new(Subdir::Found(SubdirData::from_client(client)));
                    box_future(async move {
                        barrier.set(subdir.clone()).expect("subdir was set twice");
                        Ok(subdir)
                    })
                }
            };
            pending_subdirs.push(pending);
        }

        let result_len = subdir_handles.len() + direct_url_offset;

        Ok(Self {
            gateway,
            recursive,
            reporter,
            direct_url_specs,
            pending_pattern_specs,
            pattern_names_seen,
            seen,
            pending_package_specs,
            active_extras,
            fetched: ahash::HashMap::default(),
            subdir_handles,
            pending_subdirs,
            pending_records: FuturesUnordered::new(),
            result: vec![RepoData::default(); result_len],
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

                // Push the direct url in the first subdir result for channel priority logic
                let (unique_base_deps, unique_extra_deps) =
                    super::subdir::extract_unique_deps_split(records.iter().map(|r| &**r));
                Ok((
                    0,
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
        for (package_name, request) in self.pending_package_specs.drain() {
            for handle in &self.subdir_handles {
                let request = request.clone();
                let package_name = package_name.clone();
                let reporter = self.reporter.clone();
                let result_index = handle.result_index;
                let barrier = handle.barrier.clone();

                self.pending_records.push(box_future(async move {
                    let subdir = barrier.wait().await;
                    match subdir.as_ref() {
                        Subdir::Found(subdir) => subdir
                            .get_or_fetch_package_records(&package_name, reporter)
                            .await
                            .map(|pkg| (result_index, request, pkg)),
                        Subdir::NotFound => Ok((result_index, request, PackageRecords::default())),
                    }
                }));
            }
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

    /// Add matching records to the result.
    fn accumulate_records(
        &mut self,
        result_idx: usize,
        records: Vec<Arc<RepoDataRecord>>,
        request: &PendingRequest,
    ) {
        let result = &mut self.result[result_idx];

        match &request.specs {
            SourceSpecs::Transitive => {
                // All records match — extend with Arc clones (cheap refcount bumps).
                result.records.extend(records);
            }
            SourceSpecs::Input(specs) => {
                // Only a subset matches — filter and clone matching Arcs.
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
                    let subdir = subdir_result?;
                    self.expand_pattern_specs_for_subdir(subdir.as_ref());
                    if self.pending_subdirs.is_empty() {
                        self.pending_pattern_specs.clear();
                        self.pattern_names_seen.clear();
                    }
                }

                // Handle any records that were fetched
                records = self.pending_records.select_next_some() => {
                    let (result_idx, request, pkg) = records?;

                    if self.recursive {
                        // Cache for late activations, then walk deps.
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

                    self.accumulate_records(result_idx, pkg.records, &request);
                }

                // All futures have been handled, all subdirectories have been loaded and all
                // repodata records have been fetched
                complete => {
                    break;
                }
            }
        }

        Ok(self.result)
    }
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
type PendingSubdirResult = Result<Arc<Subdir>, GatewayError>;
type PendingRecordsResult = Result<(usize, PendingRequest, PackageRecords), GatewayError>;

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

    /// Execute the query and return the package names.
    pub async fn execute(self) -> Result<Vec<PackageName>, GatewayError> {
        // Collect all the channels and platforms together
        let channels_and_platforms = self
            .channels
            .iter()
            .cartesian_product(self.platforms)
            .collect_vec();

        // Create barrier cells for each subdirectory.
        // This can be used to wait until the subdir becomes available.
        let mut pending_subdirs = FuturesUnordered::new();
        for (channel, platform) in channels_and_platforms {
            // Create a barrier so work that need this subdir can await it.
            // Set the subdir to prepend the direct url queries in the result.

            let inner = self.gateway.clone();
            let reporter = self.reporter.clone();
            pending_subdirs.push(async move {
                match inner
                    .get_or_create_subdir(channel, platform, reporter)
                    .await
                {
                    Ok(subdir) => Ok(subdir.package_names().unwrap_or_default()),
                    Err(e) => Err(e),
                }
            });
        }
        let mut names: std::collections::HashSet<String> = std::collections::HashSet::default();

        while let Some(result) = pending_subdirs.next().await {
            let subdir_names = result?;
            names.extend(subdir_names);
        }

        Ok(names
            .into_iter()
            .map(PackageName::try_from)
            .collect::<Result<Vec<PackageName>, _>>()?)
    }
}

impl IntoFuture for NamesQuery {
    type Output = Result<Vec<PackageName>, GatewayError>;
    type IntoFuture = BoxFuture<Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        box_future(self.execute())
    }
}
