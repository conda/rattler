use std::{
    collections::{HashMap, HashSet},
    future::{Future, IntoFuture},
    sync::Arc,
};

use futures::{select_biased, stream::FuturesUnordered, FutureExt, StreamExt};
use itertools::Itertools;
use rattler_conda_types::{Channel, MatchSpec, Matches, PackageName, Platform};

use super::{subdir::Subdir, BarrierCell, GatewayError, GatewayInner, RepoData};
use crate::Reporter;

/// Represents a query to execute with a [`Gateway`].
///
/// When executed the query will asynchronously load the repodata from all
/// subdirectories (combination of channels and platforms).
///
/// Most processing will happen on the background so downloading and parsing
/// can happen simultaneously.
///
/// Repodata is cached by the [`Gateway`] so executing the same query twice
/// with the same channels will not result in the repodata being fetched
/// twice.
#[derive(Clone)]
pub struct RepoDataQuery {
    /// The gateway that manages all resources
    gateway: Arc<GatewayInner>,

    /// The channels to fetch from
    channels: Vec<Channel>,

    /// The platforms the fetch from
    platforms: Vec<Platform>,

    /// The specs to fetch records for
    specs: Vec<MatchSpec>,

    /// Whether to recursively fetch dependencies
    recursive: bool,

    /// The reporter to use by the query.
    reporter: Option<Arc<dyn Reporter>>,
}

#[derive(Clone)]
enum SourceSpecs {
    /// The record is required by the user.
    Input(Vec<MatchSpec>),

    /// The record is required by a dependency.
    Transitive,
}

impl RepoDataQuery {
    /// Constructs a new instance. This should not be called directly, use
    /// [`Gateway::query`] instead.
    pub(super) fn new(
        gateway: Arc<GatewayInner>,
        channels: Vec<Channel>,
        platforms: Vec<Platform>,
        specs: Vec<MatchSpec>,
    ) -> Self {
        Self {
            gateway,
            channels,
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

        // Collect all the channels and platforms together
        let channels_and_platforms = self
            .channels
            .iter()
            .cartesian_product(self.platforms.into_iter())
            .collect_vec();

        // Collect all the specs that have a direct url and the ones that have a name.
        let mut seen = HashSet::new();
        let mut pending_package_specs = HashMap::new();
        let mut direct_url_specs = vec![];
        for spec in self.specs {
            if let Some(url) = spec.url.clone() {
                let name = spec
                    .name
                    .clone()
                    .ok_or(GatewayError::MatchSpecWithoutName(Box::new(spec.clone())))?;
                seen.insert(name.clone());
                direct_url_specs.push((spec.clone(), url, name));
            } else if let Some(name) = &spec.name {
                seen.insert(name.clone());
                let pending = pending_package_specs
                    .entry(name.clone())
                    .or_insert_with(|| SourceSpecs::Input(vec![]));
                let SourceSpecs::Input(input_specs) = pending else {
                    panic!("RootSpecs::Input was overwritten by RootSpecs::Transitive");
                };
                input_specs.push(spec);
            }
        }

        // Short circuit if there are no channels or platforms specified
        if direct_url_specs.is_empty() && channels_and_platforms.is_empty() {
            return Ok(Vec::default());
        }

        // Result offset for direct url queries.
        let direct_url_offset = usize::from(!direct_url_specs.is_empty());

        // Create barrier cells for each subdirectory.
        // This can be used to wait until the subdir becomes available.
        let mut subdirs = Vec::with_capacity(channels_and_platforms.len());
        let mut pending_subdirs = FuturesUnordered::new();
        for (subdir_idx, (channel, platform)) in channels_and_platforms.into_iter().enumerate() {
            // Create a barrier so work that need this subdir can await it.
            let barrier = Arc::new(BarrierCell::new());
            // Set the subdir to prepend the direct url queries in the result.
            subdirs.push((subdir_idx + direct_url_offset, barrier.clone()));

            let inner = self.gateway.clone();
            let reporter = self.reporter.clone();
            pending_subdirs.push(async move {
                match inner
                    .get_or_create_subdir(channel, platform, reporter)
                    .await
                {
                    Ok(subdir) => {
                        barrier.set(subdir).expect("subdir was set twice");
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            });
        }

        // A list of futures to fetch the records for the pending package names.
        // The main task awaits these futures.
        let mut pending_records = FuturesUnordered::new();

        // Push the direct url queries to the pending_records.
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "wasm32")] {
                if let Some((_,first_url,_)) = direct_url_specs.into_iter().next() {
                    // Direct url queries are not supported in wasm.
                    return Err(GatewayError::DirectUrlQueryNotSupported(first_url.to_string()));
                }
            } else {
                for (spec, url, name) in direct_url_specs {
                    let gateway = self.gateway.clone();
                    pending_records.push(
                        box_future(async move {
                            let query = super::direct_url_query::DirectUrlQuery::new(
                                url.clone(),
                                gateway.package_cache.clone(),
                                gateway.client.clone(),
                                spec.sha256,
                            );

                            let record = query
                                .execute()
                                .await
                                .map_err(|e| GatewayError::DirectUrlQueryError(url.to_string(), e))?;

                            // Check if record actually has the same name
                            if let Some(record) = record.first() {
                                if record.package_record.name != name {
                                    // Using as_source to get the closest to the retrieved input.
                                    return Err(GatewayError::UrlRecordNameMismatch(
                                        record.package_record.name.as_source().to_string(),
                                        name.as_source().to_string(),
                                    ));
                                }
                            }
                            // Push the direct url in the first subdir result for channel priority logic.
                            Ok((0, SourceSpecs::Input(vec![spec]), record))
                        }),
                    );
                }
            }
        }

        let len = subdirs.len() + direct_url_offset;
        let mut result = vec![RepoData::default(); len];

        // Loop until all pending package names have been fetched.
        loop {
            // Iterate over all pending package names and create futures to fetch them from
            // all subdirs.
            for (package_name, specs) in pending_package_specs.drain() {
                for (subdir_idx, subdir) in subdirs.iter().cloned() {
                    let specs = specs.clone();
                    let package_name = package_name.clone();
                    let reporter = self.reporter.clone();
                    pending_records.push(box_future(async move {
                        let barrier_cell = subdir.clone();
                        let subdir = barrier_cell.wait().await;
                        match subdir.as_ref() {
                            Subdir::Found(subdir) => subdir
                                .get_or_fetch_package_records(&package_name, reporter)
                                .await
                                .map(|records| (subdir_idx, specs, records)),
                            Subdir::NotFound => {
                                Ok((subdir_idx + direct_url_offset, specs, Arc::from(vec![])))
                            }
                        }
                    }));
                }
            }

            // Wait for the subdir to become available.
            select_biased! {
                // Handle any error that was emitted by the pending subdirs.
                subdir_result = pending_subdirs.select_next_some() => {
                    subdir_result?;
                }

                // Handle any records that were fetched
                records = pending_records.select_next_some() => {
                    let (result_idx, request_specs, records) = records?;

                    if self.recursive {
                        // Extract the dependencies from the records and recursively add them to the
                        // list of package names that we need to fetch.
                        for record in records.iter() {
                            if let SourceSpecs::Input(request_specs) = &request_specs {
                                if !request_specs.iter().any(|spec| spec.matches(record)) {
                                    // Do not recurse into records that do not match to root spec.
                                    continue;
                                }
                            }
                            for dependency in &record.package_record.depends {
                                // Use only the name for transitive dependencies.
                                let dependency_name = PackageName::new_unchecked(
                                    dependency.split_once(' ').unwrap_or((dependency, "")).0,
                                );
                                if seen.insert(dependency_name.clone()) {
                                    pending_package_specs.insert(dependency_name.clone(), SourceSpecs::Transitive);
                                }
                            }

                            for (_, dependencies) in record.package_record.extra_depends.iter() {
                                for dependency in dependencies {
                                    let dependency_name = package_name_from_match_spec_str(dependency);
                                    if seen.insert(dependency_name.clone()) {
                                        pending_package_specs.insert(dependency_name.clone(), SourceSpecs::Transitive);
                                    }
                                }
                            }
                        }
                    }

                    // Add the records to the result
                    if records.len() > 0 {
                        let result = &mut result[result_idx];

                        for record in records.iter() {
                            if let SourceSpecs::Input(request_specs) = &request_specs {
                                if !request_specs.iter().any(|spec| spec.matches(record)) {
                                    // Do not return records that do not match to input spec.
                                    continue;
                                }
                            }
                            result.len += 1;
                            result.shards.push(Arc::new([record.clone()]));
                        }
                    }
                }

                // All futures have been handled, all subdirectories have been loaded and all
                // repodata records have been fetched.
                complete => {
                    break;
                }
            }
        }

        Ok(result)
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
        let mut names: HashSet<String> = HashSet::default();

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

fn package_name_from_match_spec_str(spec: &str) -> PackageName {
    let package_name_str = spec
        .split_once(|c: char| c.is_whitespace() || matches!(c, '>' | '<' | '=' | '!' | '~'))
        .map_or(spec, |(name, _)| name);
    PackageName::new_unchecked(package_name_str)
}

#[cfg(test)]
mod test {
    use rstest::*;

    #[rstest]
    #[case("pillow", "pillow")]
    #[case("pillow >=10", "pillow")]
    #[case("pillow>=10,<12", "pillow")]
    #[case("pillow >=10, <12", "pillow")]
    fn test_package_name_from_match_spec_str(#[case] spec: &str, #[case] expected: &str) {
        let package_name = super::package_name_from_match_spec_str(spec);
        assert_eq!(package_name.as_source(), expected);
    }
}
