use crate::pool::StringId;
use crate::solvable::{Solvable, SolvableId};
use rattler_conda_types::{MatchSpec, PackageRecord, Version};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::str::FromStr;

/// Returns the order of two candidates based on rules used by conda.
pub(crate) fn compare_candidates(
    solvables: &[Solvable],
    interned_strings: &HashMap<String, StringId>,
    packages_by_name: &HashMap<StringId, Vec<SolvableId>>,
    a: &PackageRecord,
    b: &PackageRecord,
) -> Ordering {
    // First compare by "tracked_features". If one of the packages has a tracked feature it is
    // sorted below the one that doesn't have the tracked feature.
    let a_has_tracked_features = a.track_features.is_empty();
    let b_has_tracked_features = b.track_features.is_empty();
    match b_has_tracked_features.cmp(&a_has_tracked_features) {
        Ordering::Less => return Ordering::Less,
        Ordering::Greater => return Ordering::Greater,
        Ordering::Equal => {}
    };

    // Otherwise, select the variant with the highest version
    match a.version.cmp(&b.version) {
        Ordering::Less => return Ordering::Greater,
        Ordering::Greater => return Ordering::Less,
        Ordering::Equal => {}
    };

    // Otherwise, select the variant with the highest build number
    match a.build_number.cmp(&b.build_number) {
        Ordering::Less => return Ordering::Greater,
        Ordering::Greater => return Ordering::Less,
        Ordering::Equal => {}
    };

    // Otherwise, compare the dependencies of the variants. If there are similar
    // dependencies select the variant that selects the highest version of the dependency.
    let a_match_specs: Vec<_> = a
        .depends
        .iter()
        .map(|d| MatchSpec::from_str(d).unwrap())
        .collect();
    let b_match_specs: Vec<_> = b
        .depends
        .iter()
        .map(|d| MatchSpec::from_str(d).unwrap())
        .collect();

    let b_specs_by_name: HashMap<_, _> = b_match_specs
        .iter()
        .filter_map(|spec| spec.name.as_ref().map(|name| (name, spec)))
        .collect();

    let a_specs_by_name = a_match_specs
        .iter()
        .filter_map(|spec| spec.name.as_ref().map(|name| (name, spec)));

    let mut total_score = 0;
    for (a_dep_name, a_spec) in a_specs_by_name {
        if let Some(b_spec) = b_specs_by_name.get(&a_dep_name) {
            if &a_spec == b_spec {
                continue;
            }

            // Find which of the two specs selects the highest version
            let highest_a =
                find_highest_version(solvables, interned_strings, packages_by_name, a_spec);
            let highest_b =
                find_highest_version(solvables, interned_strings, packages_by_name, b_spec);

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
            // weight that variant.
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
    b.timestamp.cmp(&a.timestamp)
}

pub(crate) fn find_highest_version(
    solvables: &[Solvable],
    interned_strings: &HashMap<String, StringId>,
    packages_by_name: &HashMap<StringId, Vec<SolvableId>>,
    match_spec: &MatchSpec,
) -> Option<(Version, bool)> {
    let name = match_spec.name.as_deref().unwrap();
    let name_id = interned_strings[name];

    // For each record that matches the spec
    let candidates = packages_by_name[&name_id]
        .iter()
        .map(|s| solvables[s.index()].package().record)
        .filter(|s| match_spec.matches(s));

    candidates.fold(None, |init, record| {
        Some(init.map_or_else(
            || (record.version.clone(), !record.track_features.is_empty()),
            |(version, has_tracked_features)| {
                (
                    version.max(record.version.clone()),
                    has_tracked_features && record.track_features.is_empty(),
                )
            },
        ))
    })
}
