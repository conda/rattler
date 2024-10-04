use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
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

    fn solvable_record(&self, id: SolvableId) -> &SolverPackageRecord<'repo> {
        let pool = &self.solver.provider().pool;
        let solvable = pool.resolve_solvable(id);

        &solvable.record
    }

    /// This function can be used for the initial sorting of the candidates.
    pub fn sort_by_name_version_build(&self, solvables: &mut [SolvableId]) {
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

    pub fn sort_by_dependencies(
        &self,
        solvables: &mut [SolvableId],
        version_cache: &mut HashMap<VersionSetId, Option<(Version, bool)>>,
    ) {
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
                self.sort_by_highest_version(sub, version_cache);
            }

            start = end;
        }
    }

    /// Sorts the solvables by the highest version of the dependencies shared by the solvables.
    /// what this function does is:
    /// 1. Find the first unsorted solvable in the list
    /// 2. Get the dependencies for each solvable
    /// 3. Get the known dependencies for each solvable, filter out the unknown dependencies
    /// 4. Retain the dependencies that are shared by all the solvables
    /// 5. Create a max vector which is the maximum version of each of the shared dependencies, per dependency name
    /// 6. Calculate a total score  by counting the position of the solvable in the list with sorted dependencies
    /// 7. Sort by the total score and use timestamp of the record as a tie breaker
    fn sort_by_highest_version(
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
                    Requirement::Union(_) => {
                        todo!("Union requirements, are not implemented in the ordering")
                    }
                });
                (*i, dep_ids.collect::<HashSet<_>>())
            })
            .collect_vec();

        // Unique names that all entries have in common
        let unique_names: HashSet<_> = unique_name_ids(
            id_and_deps
                .iter()
                .map(|(_, names)| names.iter().map(|(name, _)| *name).collect()),
        );

        // Only retain the dependencies for each solvable that are shared by all solvables
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

        let scores =
            DependencyScores::from_dependencies(shared_dependencies, self.solver, version_cache)
                .score_solvables();

        // Sort by the total score and use timestamp of the record as a tie breaker
        solvables.sort_by(|a, b| {
            // Sort by the calculated score
            let a_score = scores.get(a).unwrap_or(&0);
            let b_score = scores.get(b).unwrap_or(&0);

            // Reverse the order, so that the highest score is first
            b_score.cmp(a_score).then_with(|| {
                let a_record = self.solvable_record(*a);
                let b_record = self.solvable_record(*b);
                b_record.timestamp().cmp(&a_record.timestamp())
            })
        });
    }
}

/// Maximum version of a dependency that is shared by all solvables
#[derive(Debug, Clone)]
struct MaxSolvable {
    // The version of the dependency
    // Only sort by version
    version: Option<TrackedFeatureVersion>,
    solvable_id: SolvableId,
}

impl MaxSolvable {
    fn new(version: Option<TrackedFeatureVersion>, solvable_id: SolvableId) -> Self {
        Self {
            version,
            solvable_id,
        }
    }
}

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
}

impl Ord for TrackedFeatureVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.version.cmp(&other.version) {
            Ordering::Equal => match (self.tracked_features, other.tracked_features) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => Ordering::Equal,
            },
            other => other,
        }
    }
}

impl PartialOrd for TrackedFeatureVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A struct that calculates the score for each solvable based on the dependencies
struct DependencyScores {
    max_map: HashMap<NameId, Vec<MaxSolvable>>,
}

impl DependencyScores {
    fn from_dependencies(
        shared_dependencies: Vec<(SolvableId, HashMap<NameId, VersionSetId>)>,
        solver: &SolverCache<CondaDependencyProvider<'_>>,
        highest_version_cache: &mut HashMap<VersionSetId, Option<(Version, bool)>>,
    ) -> Self {
        // Map with the maximum version per name
        let mut max_map = HashMap::new();
        for (solvable, dependencies) in shared_dependencies {
            for (name, version_set_id) in dependencies {
                let version = find_highest_version(version_set_id, solver, highest_version_cache)
                    .map(|v| TrackedFeatureVersion::new(v.0, v.1));
                // Update the max version for the name
                let max_solvable = MaxSolvable::new(version, solvable);
                max_map
                    .entry(name)
                    .and_modify(|v: &mut Vec<MaxSolvable>| {
                        v.push(max_solvable.clone());
                    })
                    .or_insert_with(|| vec![max_solvable]);
            }
        }

        // Sort all vectors of dependencies by version
        for max_solvables in max_map.values_mut() {
            max_solvables.sort_by(|a, b| a.version.cmp(&b.version));
            // dbg!(max_solvables);
        }

        Self { max_map }
    }

    /// Per dependency, score the solvables based on the highest version of the dependency
    fn score_solvables(&self) -> HashMap<SolvableId, u32> {
        let mut scores = HashMap::new();
        // Create a score per dependency name, how high it is ranked in the list
        for (_, solvables) in self.max_map.iter() {
            let mut score = 0;
            let mut last_version = None;
            for solvable in solvables {
                // No score if there is no version
                // These should be at the beginning of the list
                if solvable.version.is_none() {
                    continue;
                }
                // Increase the score if the version is different from the previous one
                if last_version != solvable.version.as_ref() {
                    score += 1;
                }
                // Add the score to the solvable
                scores
                    .entry(solvable.solvable_id)
                    .and_modify(|v| *v += score)
                    .or_insert(score);
                last_version = solvable.version.as_ref();
            }
        }

        scores
    }
}

/// Get the unique package names from a list of vectors of package names.
fn unique_name_ids<'a>(vectors: impl IntoIterator<Item = HashSet<NameId>>) -> HashSet<NameId> {
   iter
   	.into_iter()
 	.reduce(|mut acc, hs| {
            acc.retain(|name| hs.contains(name));
            acc
        })
        .unwrap_or_default()
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
