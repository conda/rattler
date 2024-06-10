use std::{cmp::Ordering, collections::HashMap};

use futures::future::FutureExt;
use rattler_conda_types::Version;
use resolvo::{Dependencies, SolvableId, SolverCache, VersionSetId};

use crate::resolvo::CondaDependencyProvider;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum CompareStrategy {
    Default,
    LowestVersion,
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

    // return Ordering::Equal;

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
                .map(|id| (*id, pool.resolve_version_set(*id)))
                .map(|(spec_id, _)| (pool.resolve_version_set_package_name(spec_id), spec_id))
                .collect::<HashMap<_, _>>();

            let b_match_specs = b_known
                .requirements
                .iter()
                .map(|id| (*id, pool.resolve_version_set(*id)))
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
