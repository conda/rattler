use std::{
    collections::{HashMap, HashSet},
    future::IntoFuture,
    sync::Arc,
};

use futures::{select_biased, stream::FuturesUnordered, FutureExt, StreamExt};
use itertools::Itertools;
use rattler_conda_types::{Channel, MatchSpec, Matches, PackageName, Platform};

use super::{subdir::Subdir, BarrierCell, GatewayError, GatewayInner, RepoData};
use crate::{gateway::direct_url_query::DirectUrlQuery, Reporter};

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
pub struct GatewayQuery {
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

impl GatewayQuery {
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
        // Collect all the channels and platforms together
        let channels_and_platforms = self
            .channels
            .iter()
            .cartesian_product(self.platforms.into_iter())
            .collect_vec();

        // Package names that we have or will issue requests for.
        let mut seen = HashSet::new();
        let mut pending_package_specs = HashMap::new();
        let mut direct_url_specs = vec![];
        for spec in self.specs {
            if spec.url.is_some() {
                let name = spec
                    .name
                    .clone()
                    .ok_or(GatewayError::MatchSpecWithoutName(spec.clone()))?;
                seen.insert(name.clone());
                direct_url_specs.push(spec);
            } else if let Some(name) = &spec.name {
                seen.insert(name.clone());
                pending_package_specs
                    .entry(name.clone())
                    .or_insert_with(Vec::new)
                    .push(spec);
            }
        }

        // Prepare subdir vec, with direct url offset prepended.
        let direct_url_offset = if direct_url_specs.is_empty() { 0 } else { 1 };
        let mut subdirs = Vec::with_capacity(channels_and_platforms.len() + direct_url_offset);

        // Create barrier cells for each subdirectory.
        // This can be used to wait until the subdir becomes available.
        let mut pending_subdirs = FuturesUnordered::new();
        for (subdir_idx, (channel, platform)) in channels_and_platforms.into_iter().enumerate() {
            // Create a barrier so work that need this subdir can await it.
            let barrier = Arc::new(BarrierCell::new());
            subdirs.push((subdir_idx, barrier.clone()));

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

        let len = subdirs.len();
        let mut result = vec![RepoData::default(); len];

        // A list of futures to fetch the records for the pending package names.
        // The main task awaits these futures.
        let mut pending_records = FuturesUnordered::new();

        // Push the direct url queries to the pending_records.
        for spec in direct_url_specs.clone() {
            let gateway = self.gateway.clone();
            pending_records.push(
                async move {
                    let url = spec
                        .clone()
                        .url
                        .ok_or(GatewayError::MatchSpecWithoutName(spec.clone()))?;
                    let query = DirectUrlQuery::new(
                        url.clone(),
                        gateway.package_cache.clone(),
                        gateway.client.clone(),
                    );

                    let record = query
                        .execute()
                        .await
                        .map_err(|e| GatewayError::DirectUrlQueryError(url.to_string(), e))?;

                    // Check if record actually has the same name
                    if let Some(record) = record.first() {
                        let spec_name = spec.clone().name.ok_or(GatewayError::MatchSpecWithoutName(spec.clone()))?;
                        if record.package_record.name != spec_name {
                            // Using as_source to get the closest to the retrieved input.
                            return Err(GatewayError::NotMatchingNameUrl(
                                record.package_record.name.as_source().to_string(),
                                spec_name.as_source().to_string(),
                            ));
                        }
                    }
                    // Push the direct url in the first subdir for channel priority logic.
                    Ok((0, vec![spec], record))
                }
                    .boxed(),
            );
        }

        // Loop until all pending package names have been fetched.
        loop {
            // Iterate over all pending package names and create futures to fetch them from
            // all subdirs.
            for (package_name, specs) in pending_package_specs.drain() {
                for (subdir_idx, subdir) in subdirs.iter().cloned() {
                    let specs = specs.clone();
                    let package_name = package_name.clone();
                    let reporter = self.reporter.clone();
                    pending_records.push(
                        async move {
                            let barrier_cell = subdir.clone();
                            let subdir = barrier_cell.wait().await;
                            match subdir.as_ref() {
                                Subdir::Found(subdir) => subdir
                                    .get_or_fetch_package_records(&package_name, reporter)
                                    .await
                                    .map(|records| (subdir_idx + direct_url_offset, specs, records)),
                                Subdir::NotFound => Ok((subdir_idx + direct_url_offset, specs, Arc::from(vec![]))),
                            }
                        }
                            .boxed(),
                    );
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
                    let (subdir_idx, request_specs, records) = records?;

                    if self.recursive {
                        // Extract the dependencies from the records and recursively add them to the
                        // list of package names that we need to fetch.
                        for record in records.iter() {
                            if !request_specs.iter().any(|spec| spec.matches(record)) {
                                // Do not recurse into records that do not match to root spec.
                                continue;
                            }
                            for dependency in &record.package_record.depends {
                                // Use only the name for transitive dependencies.
                                let dependency_name = PackageName::new_unchecked(
                                    dependency.split_once(' ').unwrap_or((dependency, "")).0,
                                );
                                if seen.insert(dependency_name.clone()) {
                                    pending_package_specs.insert(dependency_name.clone(), vec![dependency_name.into()]);
                                }
                            }
                        }
                    }

                    // Add the records to the result
                    if records.len() > 0 {
                        let result = &mut result[subdir_idx];
                        result.len += records.len();
                        result.shards.push(records);
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

impl IntoFuture for GatewayQuery {
    type Output = Result<Vec<RepoData>, GatewayError>;
    type IntoFuture = futures::future::BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        self.execute().boxed()
    }
}
