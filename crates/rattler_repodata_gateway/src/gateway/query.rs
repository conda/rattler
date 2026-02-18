use std::{
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

    // Mutable state during execution
    /// Normalized (lowercase) package names we've already queued.
    seen: hashbrown::HashMap<String, (), ahash::RandomState>,
    pending_package_specs: ahash::HashMap<PackageName, SourceSpecs>,

    // Subdir management
    subdir_handles: Vec<SubdirHandle>,
    pending_subdirs: FuturesUnordered<BoxFuture<Result<(), GatewayError>>>,

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
        let mut pending_package_specs = ahash::HashMap::default();
        let mut direct_url_specs = Vec::new();

        // Categorize specs into direct_url_specs and pending_package_specs
        // TODO: allow glob/regex package names as well
        for spec in specs {
            if let Some(url) = spec.url.clone() {
                let name = spec
                    .name
                    .clone()
                    .and_then(Option::<PackageName>::from)
                    .ok_or(GatewayError::MatchSpecWithoutExactName(Box::new(
                        spec.clone(),
                    )))?;
                seen.insert(name.as_normalized().to_string(), ());
                direct_url_specs.push(DirectUrlSpec { spec, url, name });
            } else if let Some(PackageNameMatcher::Exact(name)) = &spec.name {
                seen.insert(name.as_normalized().to_string(), ());
                let pending = pending_package_specs
                    .entry(name.clone())
                    .or_insert_with(|| SourceSpecs::Input(vec![]));
                let SourceSpecs::Input(input_specs) = pending else {
                    panic!("SourceSpecs::Input was overwritten by SourceSpecs::Transitive");
                };
                input_specs.push(spec);
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

            match source {
                Source::Channel(channel) => {
                    let inner = gateway.clone();
                    let reporter = reporter.clone();
                    pending_subdirs.push(box_future(async move {
                        match inner
                            .get_or_create_subdir(&channel, platform, reporter)
                            .await
                        {
                            Ok(subdir) => {
                                barrier.set(subdir).expect("subdir was set twice");
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    }));
                }
                Source::Custom(custom_source) => {
                    // For custom sources, create an adapter that wraps the source
                    // for the specific platform
                    let client = CustomSourceClient::new(custom_source, platform);
                    let subdir = Arc::new(Subdir::Found(SubdirData::from_client(client)));
                    barrier.set(subdir).expect("subdir was set twice");
                }
            }
        }

        let result_len = subdir_handles.len() + direct_url_offset;

        Ok(Self {
            gateway,
            recursive,
            reporter,
            direct_url_specs,
            seen,
            pending_package_specs,
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
                );

                let records = query
                    .execute()
                    .await
                    .map_err(|e| GatewayError::DirectUrlQueryError(url.to_string(), e))?;

                // Check if record actually has the same name
                if let Some(record) = records.first() {
                    if record.package_record.name != name {
                        return Err(GatewayError::UrlRecordNameMismatch(
                            record.package_record.name.as_source().to_string(),
                            name.as_source().to_string(),
                        ));
                    }
                }

                // Push the direct url in the first subdir result for channel priority logic
                let unique_deps = super::subdir::extract_unique_deps(records.iter().map(|r| &**r));
                Ok((
                    0,
                    SourceSpecs::Input(vec![spec]),
                    PackageRecords {
                        records,
                        unique_deps,
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
        for (package_name, specs) in self.pending_package_specs.drain() {
            for handle in &self.subdir_handles {
                let specs = specs.clone();
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
                            .map(|pkg| (result_index, specs, pkg)),
                        Subdir::NotFound => Ok((result_index, specs, PackageRecords::default())),
                    }
                }));
            }
        }
    }

    /// Extract dependencies from records and queue them if not seen.
    fn queue_dependencies(&mut self, pkg: &PackageRecords, request_specs: &SourceSpecs) {
        match request_specs {
            SourceSpecs::Transitive => {
                // Use precomputed unique deps — typically ~50-100 strings
                // instead of iterating all records (~20,000 dep strings).
                for dep in pkg.unique_deps.iter() {
                    self.queue_dependency(dep);
                }
            }
            SourceSpecs::Input(specs) => {
                // For input specs, only process deps from matching records.
                for record in &pkg.records {
                    if !specs.iter().any(|s| s.matches(record.as_ref())) {
                        continue;
                    }
                    for dependency in &record.package_record.depends {
                        self.queue_dependency(dependency);
                    }
                    for (_, dependencies) in record.package_record.experimental_extra_depends.iter()
                    {
                        for dependency in dependencies {
                            self.queue_dependency(dependency);
                        }
                    }
                }
            }
        }
    }

    /// Queue a single dependency if not already seen.
    ///
    /// Uses `entry_ref` for a single hash lookup. Only allocates when the
    /// name is genuinely new (~500 unique names vs ~1M+ dependency strings).
    fn queue_dependency(&mut self, dependency: &str) {
        let normalized = PackageName::normalized_name_from_matchspec_str(dependency);
        let normalized_str: &str = &normalized;
        if let hashbrown::hash_map::EntryRef::Vacant(entry) = self.seen.entry_ref(normalized_str) {
            entry.insert(());
            let dependency_name = PackageName::from_matchspec_str_unchecked(dependency);
            self.pending_package_specs
                .insert(dependency_name, SourceSpecs::Transitive);
        }
    }

    /// Add matching records to the result.
    fn accumulate_records(
        &mut self,
        result_idx: usize,
        records: Vec<Arc<RepoDataRecord>>,
        request_specs: &SourceSpecs,
    ) {
        let result = &mut self.result[result_idx];

        match request_specs {
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

    /// Run the main event loop.
    async fn run(mut self) -> Result<Vec<RepoData>, GatewayError> {
        self.spawn_direct_url_fetches()?;

        loop {
            self.spawn_package_fetches();

            select_biased! {
                // Handle any error that was emitted by the pending subdirs
                subdir_result = self.pending_subdirs.select_next_some() => {
                    subdir_result?;
                }

                // Handle any records that were fetched
                records = self.pending_records.select_next_some() => {
                    let (result_idx, request_specs, pkg) = records?;

                    if self.recursive {
                        self.queue_dependencies(&pkg, &request_specs);
                    }

                    self.accumulate_records(result_idx, pkg.records, &request_specs);
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
type PendingRecordsResult = Result<(usize, SourceSpecs, PackageRecords), GatewayError>;

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
            .cartesian_product(self.platforms.into_iter())
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
