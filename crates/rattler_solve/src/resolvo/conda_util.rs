use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    ops::Deref,
};

use futures::future::FutureExt;
use itertools::Itertools;
use rattler_conda_types::Version;
use resolvo::{Dependencies, NameId, Requirement, SolvableId, SolverCache, VersionSetId};

use crate::resolvo::CondaDependencyProvider;

use super::SolverPackageRecord;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum CompareStrategy {
    Default,
    LowestVersion,
}

/// Sorts the candidates based on the strategy.
/// and some different rules
pub struct SolvableSorter<'a, 'repo> {
    solver: &'a SolverCache<CondaDependencyProvider<'repo>>,
    strategy: CompareStrategy,
}

impl<'a, 'repo> SolvableSorter<'a, 'repo> {
    pub fn new(
        solver: &'a SolverCache<CondaDependencyProvider<'repo>>,
        strategy: CompareStrategy,
    ) -> Self {
        Self { solver, strategy }
    }

    fn solvable_record(&self, id: SolvableId) -> SolverPackageRecord<'repo> {
        let pool = &self.solver.provider().pool;
        let solvable = pool.resolve_solvable(a);
        solvable.record
    }

    /// This function can be used for the initial sorting of the candidates.
    pub fn sort_by_name_version_build(&self, solvables: &mut [SolvableId]) {
        solvables.sort_by(|a, b| self.initial_sort(*a, *b));
    }

    /// Sort the candidates based on:
    /// 1. Whether the package has tracked features
    /// 2. The version of the package
    /// 3. The build number of the package
    fn initial_sort(&self, a: SolvableId, b: SolvableId) -> Ordering {
        let a_record = &self.solvable_record(a);
        let b_record = &self.solvable_record(b);

        // First compare by "tracked_features". If one of the packages has a tracked
        // feature it is sorted below the one that doesn't have the tracked feature.
        let a_has_tracked_features = !a_record.track_features().is_empty();
        let b_has_tracked_features = !b_record.track_features().is_empty();
        match a_has_tracked_features.cmp(&b_has_tracked_features) {
            Ordering::Less => return Ordering::Less,
            Ordering::Greater => return Ordering::Greater,
            Ordering::Equal => {}
        };

        // Otherwise, select the variant with the highest version
        match (self.strategy, a_record.version().cmp(b_record.version())) {
            (CompareStrategy::Default, Ordering::Greater)
            | (CompareStrategy::LowestVersion, Ordering::Less) => return Ordering::Less,
            (CompareStrategy::Default, Ordering::Less)
            | (CompareStrategy::LowestVersion, Ordering::Greater) => return Ordering::Greater,
            (_, Ordering::Equal) => {}
        };

        // Otherwise, select the variant with the highest build number
        match a_record.build_number().cmp(&b_record.build_number()) {
            Ordering::Less => return Ordering::Greater,
            Ordering::Greater => return Ordering::Less,
            Ordering::Equal => return Ordering::Equal,
        };
    }

    fn find_first_unsorted(&self, solvables: &[SolvableId]) -> Option<usize> {
        // Find the first solvable record pair that have the same, name, version and build number
        // and return its index, this assumes that solvables have been sorted by name, version and build number
        for (i, solvable) in solvables.iter().enumerate() {
            if i + 1 < solvables.len() {
                let next_solvable = solvables[i + 1];
                let solvable_record = self.solvable_record(*solvable);
                let next_solvable_record = self.solvable_record(next_solvable);

                if solvable_record.name() == next_solvable_record.name()
                    && solvable_record.version() == next_solvable_record.version()
                    && solvable_record.build_number() == next_solvable_record.build_number()
                {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Sorts the solvables by the highest version of the dependencies shared by the solvables.
    /// what this function does is:
    /// 1. Find the first unsorted solvable in the list
    /// 2. Get the dependencies for each solvable
    /// 3. Get the known dependencies for each solvable, filter out the unknown dependencies
    /// 4. Retain the dependencies that are shared by all the solvables
    /// 5. Create a max vector which is the maximum version of each of the shared dependencies
    /// 6. Calculate a total score  by counting how often the solvable has a dependency that is in the max vector
    /// 7. Sort by the total score and use timestamp of the record as a tie breaker
    pub(crate) fn sort_by_highest_version(
        &self,
        solvables: &mut [SolvableId],
        highest_version_spec: &HashMap<VersionSetId, Option<(Version, bool)>>,
    ) {
        let first_unsorted = self.find_first_unsorted(solvables);
        let first_unsorted = match first_unsorted {
            Some(i) => i,
            None => return,
        };

        // Split the solvables into two parts, the ordered and the ones that need ordering
        let (_, needs_ordering) = solvables.split_at_mut(first_unsorted);

        // Get the dependencies for each solvable
        let dependencies = needs_ordering
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

        // Get the known dependencies for each solvable, filter out the unknown dependencies
        let id_and_deps = dependencies
            .into_iter()
            // Only consider known dependencies
            .filter_map(|(i, deps)| match deps {
                Dependencies::Known(known_dependencies) => Some((i, known_dependencies)),
                Dependencies::Unknown(_) => None,
            })
            .map(|(i, known)| {
                // Map all known dependencies to the package names
                let dep_ids = known.requirements.iter().filter_map(|req| match req {
                    Requirement::Single(version_set_id) => Some((
                        self.solver
                            .provider()
                            .pool
                            .resolve_version_set_package_name(*version_set_id),
                        *version_set_id,
                    )),
                    // Ignore union requirements
                    Requirement::Union(_) => None,
                });
                (i, dep_ids.collect::<HashSet<_>>())
            })
            .collect_vec();

        let unique_names: HashSet<_> = unique_name_ids(
            id_and_deps
                .iter()
                .map(|(_, names)| names.iter().map(|(name, _)| *name).collect()),
        );

        // Only retain the dependencies that are shared by all solvables
        let shared_dependencies = id_and_deps
            .into_iter()
            .map(|(i, names)| {
                (
                    i,
                    names
                        .into_iter()
                        .filter(|(name, _)| unique_names.contains(name))
                        .collect::<HashMap<_, VersionSetId>>(),
                )
            })
            .collect_vec();

        // Map the shared dependencies to the highest version of each dependency

        // Get the set of dependencies that each solvable has
    }
}

struct Sorter {
    max_map: HashMap<NameId, Version>,

}

fn max_transforms() ->


// TODO: remove once we have make NameId Ord
//
#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
struct NameIdWrapper(pub NameId);

impl Deref for NameIdWrapper {
    type Target = NameId;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Ord for NameIdWrapper {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0 .0.cmp(&other.0 .0)
    }
}

impl PartialOrd for NameIdWrapper {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Get the unique package names from a list of vectors of package names.
fn unique_name_ids<'a>(vectors: impl IntoIterator<Item = HashSet<NameId>>) -> HashSet<NameId> {
    let mut iter = vectors.into_iter();
    if let Some(first_set) = iter.next() {
        iter.fold(first_set.clone(), |mut acc: HashSet<NameId>, set| {
            acc.retain(|item| set.contains(item));
            acc
        })
    } else {
        HashSet::new() // Return empty set if input is empty
    }
}
/// Returns the order of two candidates based on the order used by conda.
#[allow(clippy::too_many_arguments)]
pub(super) fn compare_candidates(
    a: SolvableId,
    b: SolvableId,
    solver: &SolverCache<CondaDependencyProvider<'_>>,
    match_spec_highest_version: &mut HashMap<
        VersionSetId,
        Option<(rattler_conda_types::Version, bool)>,
    >,
    strategy: CompareStrategy,
) -> Ordering {
    let pool = &solver.provider().pool;

    let a_solvable = pool.resolve_solvable(a);
    let b_solvable = pool.resolve_solvable(b);

    let a_record = &a_solvable.record;
    let b_record = &b_solvable.record;

    // First compare by "tracked_features". If one of the packages has a tracked
    // feature it is sorted below the one that doesn't have the tracked feature.
    let a_has_tracked_features = !a_record.track_features().is_empty();
    let b_has_tracked_features = !b_record.track_features().is_empty();
    match a_has_tracked_features.cmp(&b_has_tracked_features) {
        Ordering::Less => return Ordering::Less,
        Ordering::Greater => return Ordering::Greater,
        Ordering::Equal => {}
    };

    // Otherwise, select the variant with the highest version
    match (strategy, a_record.version().cmp(b_record.version())) {
        (CompareStrategy::Default, Ordering::Greater)
        | (CompareStrategy::LowestVersion, Ordering::Less) => return Ordering::Less,
        (CompareStrategy::Default, Ordering::Less)
        | (CompareStrategy::LowestVersion, Ordering::Greater) => return Ordering::Greater,
        (_, Ordering::Equal) => {}
    };

    // Otherwise, select the variant with the highest build number
    match a_record.build_number().cmp(&b_record.build_number()) {
        Ordering::Less => return Ordering::Greater,
        Ordering::Greater => return Ordering::Less,
        Ordering::Equal => {}
    };

    // Otherwise, compare the dependencies of the variants. If there are similar
    // dependencies select the variant that selects the highest version of the
    // dependency.
    let (a_dependencies, b_dependencies) = match (
        solver
            .get_or_cache_dependencies(a)
            .now_or_never()
            .expect("get_or_cache_dependencies failed"),
        solver
            .get_or_cache_dependencies(b)
            .now_or_never()
            .expect("get_or_cache_dependencies failed"),
    ) {
        (Ok(a_deps), Ok(b_deps)) => (a_deps, b_deps),
        // If either call fails, it's likely due to solver cancellation; thus, we can't compare
        // dependencies
        _ => return Ordering::Equal,
    };

    // If the MatchSpecs are known use these
    // map these into a HashMap<PackageName, VersionSetId>
    // for comparison later
    let (a_specs_by_name, b_specs_by_name) =
        if let (Dependencies::Known(a_known), Dependencies::Known(b_known)) =
            (a_dependencies, b_dependencies)
        {
            let a_match_specs = a_known
                .requirements
                .iter()
                .filter_map(|req| match req {
                    Requirement::Single(id) => Some((*id, pool.resolve_version_set(*id))),
                    Requirement::Union(_) => None,
                })
                .map(|(spec_id, _)| (pool.resolve_version_set_package_name(spec_id), spec_id))
                .collect::<HashMap<_, _>>();

            let b_match_specs = b_known
                .requirements
                .iter()
                .filter_map(|req| match req {
                    Requirement::Single(id) => Some((*id, pool.resolve_version_set(*id))),
                    Requirement::Union(_) => None,
                })
                .map(|(spec_id, _)| (pool.resolve_version_set_package_name(spec_id), spec_id))
                .collect::<HashMap<_, _>>();
            (a_match_specs, b_match_specs)
        } else {
            (HashMap::new(), HashMap::new())
        };

    let mut total_score = 0;
    for (a_dep_name, a_spec_id) in a_specs_by_name {
        if let Some(b_spec_id) = b_specs_by_name.get(&a_dep_name) {
            if &a_spec_id == b_spec_id {
                continue;
            }

            // Find which of the two specs selects the highest version
            let highest_a = find_highest_version(a_spec_id, solver, match_spec_highest_version);
            let highest_b = find_highest_version(*b_spec_id, solver, match_spec_highest_version);

            // Skip version if no package is selected by either spec
            let (a_version, a_tracked_features, b_version, b_tracked_features) = if let (
                Some((a_version, a_tracked_features)),
                Some((b_version, b_tracked_features)),
            ) =
                (highest_a, highest_b)
            {
                (a_version, a_tracked_features, b_version, b_tracked_features)
            } else {
                continue;
            };

            // If one of the dependencies only selects versions with tracked features, down-
            // weigh that variant.
            if let Some(score) = match a_tracked_features.cmp(&b_tracked_features) {
                Ordering::Less => Some(-100),
                Ordering::Greater => Some(100),
                Ordering::Equal => None,
            } {
                total_score += score;
                continue;
            }

            // Otherwise, down-weigh the version with the lowest selected version.
            total_score += match a_version.cmp(&b_version) {
                Ordering::Less => 1,
                Ordering::Equal => 0,
                Ordering::Greater => -1,
            };
        }
    }

    // If ranking the dependencies provides a score, use that for the sorting.
    match total_score.cmp(&0) {
        Ordering::Equal => {}
        ord => return ord,
    };

    // Otherwise, order by timestamp
    b_record.timestamp().cmp(&a_record.timestamp())
}

pub(super) fn find_highest_version(
    match_spec_id: VersionSetId,
    solver: &SolverCache<CondaDependencyProvider<'_>>,
    match_spec_highest_version: &mut HashMap<
        VersionSetId,
        Option<(rattler_conda_types::Version, bool)>,
    >,
) -> Option<(Version, bool)> {
    match_spec_highest_version
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
                            (
                                version.max(record.version().clone()),
                                has_tracked_features && !record.track_features().is_empty(),
                            )
                        },
                    ))
                })
        })
        .clone()
}
