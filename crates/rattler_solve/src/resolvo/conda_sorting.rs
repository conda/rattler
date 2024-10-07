use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap},
};

use futures::future::FutureExt;
use itertools::Itertools;
use rattler_conda_types::Version;
use resolvo::{
    utils::Pool, Dependencies, NameId, Requirement, SolvableId, SolverCache, VersionSetId,
};

use super::{SolverMatchSpec, SolverPackageRecord};
use crate::resolvo::CondaDependencyProvider;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum CompareStrategy {
    Default,
    LowestVersion,
}

/// Sort the candidates based on the dependencies.
/// This sorts in two steps:
/// 1. Sort by tracked features, version, and build number
/// 2. Sort by trying to sort the solvable that selects the highest versions of
///    the shared set of dependencies
pub struct SolvableSorter<'a, 'repo> {
    solver: &'a SolverCache<CondaDependencyProvider<'repo>>,
    strategy: CompareStrategy,
    dependency_strategy: CompareStrategy,
}

impl<'a, 'repo> SolvableSorter<'a, 'repo> {
    pub fn new(
        solver: &'a SolverCache<CondaDependencyProvider<'repo>>,
        strategy: CompareStrategy,
        dependency_strategy: CompareStrategy,
    ) -> Self {
        Self {
            solver,
            strategy,
            dependency_strategy,
        }
    }

    /// Get a reference to the solvable record.
    fn solvable_record(&self, id: SolvableId) -> &SolverPackageRecord<'repo> {
        let pool = self.pool();
        let solvable = pool.resolve_solvable(id);

        &solvable.record
    }

    /// Referece to the pool
    fn pool(&self) -> &Pool<SolverMatchSpec<'repo>> {
        &self.solver.provider().pool
    }

    /// Sort the candidates based on the dependencies.
    /// This sorts in two steps:
    /// 1. Sort by tracked features, version, and build number
    /// 2. Sort by trying to find the candidate that selects the highest
    ///    versions of the shared set of dependencies
    pub fn sort(
        self,
        solvables: &mut [SolvableId],
        version_cache: &mut HashMap<VersionSetId, Option<(Version, bool)>>,
    ) {
        self.sort_by_tracked_version_build(solvables);
        self.sort_by_highest_dependency_versions(solvables, version_cache);
    }

    /// This function can be used for the initial sorting of the candidates.
    fn sort_by_tracked_version_build(&self, solvables: &mut [SolvableId]) {
        solvables.sort_by(|a, b| self.simple_compare(*a, *b));
    }

    /// Sort the candidates based on:
    /// 1. Whether the package has tracked features
    /// 2. The version of the package
    /// 3. The build number of the package
    fn simple_compare(&self, a: SolvableId, b: SolvableId) -> Ordering {
        let a_record = &self.solvable_record(a);
        let b_record = &self.solvable_record(b);

        // First compare by "tracked_features". If one of the packages has a tracked
        // feature it is sorted below the one that doesn't have the tracked feature.
        let a_has_tracked_features = !a_record.track_features().is_empty();
        let b_has_tracked_features = !b_record.track_features().is_empty();
        match (a_has_tracked_features, b_has_tracked_features) {
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            _ => {}
        };

        // Otherwise, select the variant with the highest version
        match (self.strategy, a_record.version().cmp(b_record.version())) {
            (CompareStrategy::Default, Ordering::Greater)
            | (CompareStrategy::LowestVersion, Ordering::Less) => return Ordering::Less,
            (CompareStrategy::Default, Ordering::Less)
            | (CompareStrategy::LowestVersion, Ordering::Greater) => return Ordering::Greater,
            (_, Ordering::Equal) => {}
        };

        // Otherwise, select the variant with the highest build number first
        b_record.build_number().cmp(&a_record.build_number())
    }

    fn sort_by_highest_dependency_versions(
        &self,
        solvables: &mut [SolvableId],
        version_cache: &mut HashMap<VersionSetId, Option<(Version, bool)>>,
    ) {
        // Because the list can contain multiple versions, tracked features, and builds
        // of the same package we need to create sub list of solvables that have
        // the same version, build, and tracked features and sort these sub
        // lists by the highest version of the dependencies shared by the solvables.
        let mut start = 0usize;
        let entire_len = solvables.len();
        while start < entire_len {
            let mut end = start + 1;

            // Find the range of solvables with the same: version, build, tracked features
            while end < entire_len
                && self.simple_compare(solvables[start], solvables[end]) == Ordering::Equal
            {
                end += 1;
            }

            // Take the sub list of solvables
            let sub = &mut solvables[start..end];
            if sub.len() > 1 {
                // Sort the sub list of solvables by the highest version of the dependencies
                self.sort_subset_by_highest_dependency_versions(sub, version_cache);
            }

            start = end;
        }
    }

    /// Sorts the solvables by the highest version of the dependencies shared by
    /// the solvables. what this function does is:
    /// 1. Find the first unsorted solvable in the list
    /// 2. Get the dependencies for each solvable
    /// 3. Get the known dependencies for each solvable, filter out the unknown
    ///    dependencies
    /// 4. Retain the dependencies that are shared by all the solvables
    /// 6. Calculate a total score by counting the position of the solvable in
    ///    the list with sorted dependencies
    /// 7. Sort by the score per solvable and use timestamp of the record as a
    ///    tie breaker
    fn sort_subset_by_highest_dependency_versions(
        &self,
        solvables: &mut [SolvableId],
        version_cache: &mut HashMap<VersionSetId, Option<(Version, bool)>>,
    ) {
        // Get the dependencies for each solvable
        let dependencies = solvables
            .iter()
            .map(|id| {
                self.solver
                    .get_or_cache_dependencies(*id)
                    .now_or_never()
                    .expect("get_or_cache_dependencies failed")
                    .map(|deps| (id, deps))
            })
            .collect::<Result<Vec<_>, _>>();

        let dependencies = match dependencies {
            Ok(dependencies) => dependencies,
            // Solver cancelation, lets just return
            Err(_) => return,
        };

        // Get the known dependencies for each solvable, filter out the unknown
        // dependencies
        let mut id_and_deps: HashMap<_, Vec<_>> = HashMap::with_capacity(dependencies.len());
        let mut name_count: HashMap<NameId, usize> = HashMap::new();
        for (solvable_idx, &solvable_id) in solvables.iter().enumerate() {
            let dependencies = self
                .solver
                .get_or_cache_dependencies(solvable_id)
                .now_or_never()
                .expect("get_or_cache_dependencies failed");
            let known = match dependencies {
                Ok(Dependencies::Known(known_dependencies)) => known_dependencies,
                Ok(Dependencies::Unknown(_)) => {
                    unreachable!("Unknown dependencies should never happen in the conda ecosystem")
                }
                // Solver cancelation, lets just return
                Err(_) => return,
            };

            for requirement in &known.requirements {
                let version_set_id = match requirement {
                    // Ignore union requirements, these do not occur in the conda ecosystem
                    // currently
                    Requirement::Union(_) => {
                        unreachable!("Union requirements, are not implemented in the ordering")
                    }
                    Requirement::Single(version_set_id) => version_set_id,
                };

                // Get the name of the dependency and add the version set id to the list of
                // version sets for a particular package. A single solvable can depend on a
                // single package multiple times.
                let dependency_name = self
                    .pool()
                    .resolve_version_set_package_name(*version_set_id);

                // Check how often we have seen this dependency name
                let name_count = match name_count.entry(dependency_name) {
                    Entry::Occupied(entry) if entry.get() + 1 >= solvable_idx => entry.into_mut(),
                    Entry::Vacant(entry) if solvable_idx == 0 => entry.insert(0),
                    _ => {
                        // We have already not seen this dependency name for all solvables so there
                        // is no need to allocate additional memory to track
                        // it.
                        continue;
                    }
                };

                match id_and_deps.entry((solvable_id, dependency_name)) {
                    Entry::Occupied(mut entry) => entry.get_mut().push(*version_set_id),
                    Entry::Vacant(entry) => {
                        entry.insert(vec![*version_set_id]);
                        *name_count += 1;
                    }
                }
            }
        }

        // Sort all the dependencies that the solvables have in common by their name.
        let sorted_unique_names = name_count
            .into_iter()
            .filter_map(|(name, count)| {
                if count == solvables.len() {
                    Some(name)
                } else {
                    None
                }
            })
            .sorted_by_key(|name| self.pool().resolve_package_name(*name))
            .collect_vec();

        // A closure that locates the highest version of a dependency for a solvable.
        let mut find_highest_version_for_set = |version_set_ids: &Vec<VersionSetId>| {
            version_set_ids
                .iter()
                .filter_map(|id| find_highest_version(*id, self.solver, version_cache))
                .map(|v| TrackedFeatureVersion::new(v.0, v.1))
                .fold(None, |init, version| {
                    if let Some(init) = init {
                        Some(
                            if version.compare_with_strategy(&init, CompareStrategy::Default)
                                == Ordering::Less
                            {
                                version
                            } else {
                                init
                            },
                        )
                    } else {
                        Some(version)
                    }
                })
        };

        // Sort the solvables by comparing the highest version of the shared
        // dependencies in alphabetic order.
        solvables.sort_by(|a, b| {
            for &name in sorted_unique_names.iter() {
                let a_version = id_and_deps
                    .get(&(*a, name))
                    .and_then(&mut find_highest_version_for_set);
                let b_version = id_and_deps
                    .get(&(*b, name))
                    .and_then(&mut find_highest_version_for_set);

                // Deal with the case where resolving the version set doesn't actually select a
                // version
                let (a_version, b_version) = match (a_version, b_version) {
                    // If we have a version for either solvable, but not the other, the one with the
                    // version is better.
                    (Some(_), None) => return Ordering::Less,
                    (None, Some(_)) => return Ordering::Greater,

                    // If for neither solvable the version set doesn't select a version for the
                    // dependency we skip it.
                    (None, None) => continue,

                    (Some(a), Some(b)) => (a, b),
                };

                // Compare the versions
                match a_version.compare_with_strategy(&b_version, self.dependency_strategy) {
                    Ordering::Equal => {
                        // If this version is equal, we continue with the next dependency
                        continue;
                    }
                    ordering => return ordering,
                }
            }

            // Otherwise sort by timestamp (in reverse, we want the highest timestamp first)
            let a_record = self.solvable_record(*a);
            let b_record = self.solvable_record(*b);
            b_record.timestamp().cmp(&a_record.timestamp())
        });
    }
}

/// Couples the version with the tracked features, for easier ordering
#[derive(PartialEq, Eq, Clone, Debug)]
struct TrackedFeatureVersion {
    version: Version,
    tracked_features: bool,
}

impl TrackedFeatureVersion {
    fn new(version: Version, tracked_features: bool) -> Self {
        Self {
            version,
            tracked_features,
        }
    }

    fn compare_with_strategy(&self, other: &Self, compare_strategy: CompareStrategy) -> Ordering {
        // First compare by "tracked_features". If one of the packages has a tracked
        // feature it is sorted below the one that doesn't have the tracked feature.
        match (self.tracked_features, other.tracked_features) {
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ if compare_strategy == CompareStrategy::Default => other.version.cmp(&self.version),
            _ => self.version.cmp(&other.version),
        }
    }
}

pub(super) fn find_highest_version(
    match_spec_id: VersionSetId,
    solver: &SolverCache<CondaDependencyProvider<'_>>,
    highest_version_cache: &mut HashMap<VersionSetId, Option<(rattler_conda_types::Version, bool)>>,
) -> Option<(Version, bool)> {
    highest_version_cache
        .entry(match_spec_id)
        .or_insert_with(|| {
            let candidates = solver
                .get_or_cache_matching_candidates(match_spec_id)
                .now_or_never()
                .expect("get_or_cache_matching_candidates failed");

            // Err only happens on cancellation, so we will not continue anyways
            let candidates = if let Ok(candidates) = candidates {
                candidates
            } else {
                return None;
            };

            let pool = &solver.provider().pool;

            candidates
                .iter()
                .map(|id| &pool.resolve_solvable(*id).record)
                .fold(None, |init, record| {
                    Some(init.map_or_else(
                        || {
                            (
                                record.version().clone(),
                                !record.track_features().is_empty(),
                            )
                        },
                        |(version, has_tracked_features)| {
                            if &version < record.version() {
                                (
                                    record.version().clone(),
                                    !record.track_features().is_empty(),
                                )
                            } else {
                                (version, has_tracked_features)
                            }
                        },
                    ))
                })
        })
        .clone()
}
