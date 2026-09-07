//! Ordering of search results.
//!
//! Records are ordered the way the solver orders its candidates so that the
//! first record printed for a package is the one the solver would pick:
//!
//! 1. Records without `track_features` sort before records with them.
//! 2. Higher versions sort first.
//! 3. Higher build numbers sort first.
//! 4. Records that are still equal (typically the different variants of a
//!    single build, like `py310…_5` and `py314…_5`) are ordered by the
//!    highest version they allow for each of the dependencies they share,
//!    compared alphabetically by dependency name. The variant that selects
//!    the newest dependencies sorts first.
//! 5. Finally, newer timestamps sort first.
//!
//! Step 4 mirrors `SolvableSorter` in `rattler_solve`. It needs to know which
//! records exist for the dependencies, which is what [`DependencyIndex`]
//! provides.

use std::{cmp::Ordering, collections::HashMap};

use itertools::Itertools;
use rattler_conda_types::{
    MatchSpec, Matches, PackageName, ParseMatchSpecOptions, RepoDataRecord, RepodataRevision,
    Version,
};

/// Repodata records of the dependencies of the records being sorted, indexed
/// by package name. Used to resolve which version a dependency spec like
/// `python >=3.14,<3.15.0a0` would end up selecting.
#[derive(Default)]
pub struct DependencyIndex<'a> {
    records: HashMap<PackageName, Vec<&'a RepoDataRecord>>,
    /// Cache of the highest selectable version per dependency string.
    highest_version_cache: HashMap<String, Option<TrackedFeatureVersion>>,
}

impl<'a> DependencyIndex<'a> {
    /// Builds the index from all available records.
    pub fn new(records: impl IntoIterator<Item = &'a RepoDataRecord>) -> Self {
        let mut index = HashMap::<PackageName, Vec<&'a RepoDataRecord>>::new();
        for record in records {
            index
                .entry(record.package_record.name.clone())
                .or_default()
                .push(record);
        }
        Self {
            records: index,
            highest_version_cache: HashMap::new(),
        }
    }

    /// Returns the highest version (and whether that record has tracked
    /// features) of the records matching `spec`, or `None` if no record
    /// matches. Mirrors `find_highest_version` in the solver.
    fn highest_version(
        &mut self,
        spec_str: &str,
        spec: &MatchSpec,
    ) -> Option<TrackedFeatureVersion> {
        if let Some(cached) = self.highest_version_cache.get(spec_str) {
            return cached.clone();
        }

        let name = spec.name.as_exact()?;
        let mut highest: Option<TrackedFeatureVersion> = None;
        for record in self.records.get(name).into_iter().flatten() {
            if !spec.matches(*record) {
                continue;
            }
            let version = record.package_record.version.version();
            let tracked_features = !record.package_record.track_features.is_empty();
            highest = match highest {
                Some(current) if version <= &current.version => Some(current),
                _ => Some(TrackedFeatureVersion {
                    version: version.clone(),
                    tracked_features,
                }),
            };
        }

        self.highest_version_cache
            .insert(spec_str.to_owned(), highest.clone());
        highest
    }

    /// The best version of a dependency that a record can end up with.
    ///
    /// A record can require the same package more than once, e.g. a bare
    /// `nodejs` next to the `nodejs >=26.5.1,<27.0a0` pin from a run-export.
    /// Only versions matching all of the requirements can be selected, so
    /// take the lowest of their highest versions. This is the same
    /// approximation the solver uses.
    fn best_selectable_version(
        &mut self,
        specs: &[(String, MatchSpec)],
    ) -> Option<TrackedFeatureVersion> {
        specs
            .iter()
            .filter_map(|(spec_str, spec)| self.highest_version(spec_str, spec))
            .reduce(|a, b| {
                // Better sorts first, so `Greater` means `a` is the more restrictive.
                if a.compare(&b) == Ordering::Greater {
                    a
                } else {
                    b
                }
            })
    }
}

/// Couples a version with whether the record providing it has tracked
/// features, so the two can be ordered together.
#[derive(Clone, Debug, Eq, PartialEq)]
struct TrackedFeatureVersion {
    version: Version,
    tracked_features: bool,
}

impl TrackedFeatureVersion {
    /// Orders "better" first: no tracked features before tracked features,
    /// then higher versions before lower versions.
    fn compare(&self, other: &Self) -> Ordering {
        match (self.tracked_features, other.tracked_features) {
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => other.version.cmp(&self.version),
        }
    }
}

/// Orders records by name, then by tracked features, version and build
/// number with the best record first. Everything that is still equal after
/// this needs the dependency-aware tiebreaker.
fn simple_compare(a: &RepoDataRecord, b: &RepoDataRecord) -> Ordering {
    let a = &a.package_record;
    let b = &b.package_record;
    a.name
        .cmp(&b.name)
        .then_with(|| {
            // `is_empty() == true` (no tracked features) should sort first.
            b.track_features
                .is_empty()
                .cmp(&a.track_features.is_empty())
        })
        .then_with(|| b.version.cmp(&a.version))
        .then_with(|| b.build_number.cmp(&a.build_number))
}

/// Parses the dependencies of a record into match specs, keyed by the exact
/// name of the dependency. Dependencies that cannot be parsed, that do not
/// name a single package, or that refer to virtual packages are skipped.
fn dependency_specs(record: &RepoDataRecord) -> HashMap<PackageName, Vec<(String, MatchSpec)>> {
    let mut specs: HashMap<PackageName, Vec<(String, MatchSpec)>> = HashMap::new();
    for dependency in &record.package_record.depends {
        let Ok(spec) = MatchSpec::from_str(
            dependency,
            ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
        ) else {
            continue;
        };
        if spec.is_virtual() {
            continue;
        }
        let Some(name) = spec.name.as_exact() else {
            continue;
        };
        specs
            .entry(name.clone())
            .or_default()
            .push((dependency.clone(), spec));
    }
    specs
}

/// Returns the names of the dependencies that are needed to break ties
/// between the given records, i.e. the dependencies of every record that
/// shares its name, tracked features, version and build number with another
/// record.
///
/// Fetching the repodata of these names and feeding it to a
/// [`DependencyIndex`] allows [`sort_records`] to order such records the way
/// the solver would.
pub fn tiebreak_dependency_names(records: &[&RepoDataRecord]) -> Vec<PackageName> {
    let mut sorted = records.to_vec();
    sorted.sort_by(|a, b| simple_compare(a, b));

    sorted
        .chunk_by(|a, b| simple_compare(a, b) == Ordering::Equal)
        .filter(|group| group.len() > 1)
        .flatten()
        .flat_map(|record| dependency_specs(record).into_keys())
        .unique()
        .collect()
}

/// Sorts the records by name and, within a name, by how the solver would
/// rank them (best candidate first). See the module documentation for the
/// exact ordering.
pub fn sort_records(records: &mut [&RepoDataRecord], index: &mut DependencyIndex<'_>) {
    records.sort_by(|a, b| simple_compare(a, b));

    for group in records.chunk_by_mut(|a, b| simple_compare(a, b) == Ordering::Equal) {
        if group.len() > 1 {
            sort_group_by_dependency_versions(group, index);
        }
    }
}

/// Sorts records that are equal in name, tracked features, version and build
/// number by the highest version of the dependencies they all share, and
/// finally by timestamp (newest first). Mirrors
/// `sort_subset_by_highest_dependency_versions` in the solver.
fn sort_group_by_dependency_versions(
    records: &mut [&RepoDataRecord],
    index: &mut DependencyIndex<'_>,
) {
    let specs: Vec<_> = records.iter().map(|r| dependency_specs(r)).collect();

    // Only the dependencies shared by all records can be compared.
    let shared_names: Vec<&PackageName> = specs
        .iter()
        .flat_map(HashMap::keys)
        .counts()
        .into_iter()
        .filter(|(_, count)| *count == records.len())
        .map(|(name, _)| name)
        .sorted_by_key(|name| name.as_normalized())
        .collect();

    // Per record, the best selectable version of every shared dependency.
    let best_versions: Vec<Vec<Option<TrackedFeatureVersion>>> = specs
        .iter()
        .map(|record_specs| {
            shared_names
                .iter()
                .map(|name| index.best_selectable_version(&record_specs[*name]))
                .collect()
        })
        .collect();

    let mut order: Vec<usize> = (0..records.len()).collect();
    order.sort_by(|&a, &b| {
        for (a_version, b_version) in best_versions[a].iter().zip(&best_versions[b]) {
            let ordering = match (a_version, b_version) {
                // A record whose spec selects a version beats one whose spec
                // selects nothing.
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                // Neither spec selects a version, skip this dependency.
                (None, None) => continue,
                (Some(a), Some(b)) => a.compare(b),
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }

        // Newest timestamp first.
        records[b]
            .package_record
            .timestamp
            .cmp(&records[a].package_record.timestamp)
    });

    let sorted: Vec<&RepoDataRecord> = order.into_iter().map(|i| records[i]).collect();
    records.copy_from_slice(&sorted);
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::{
        PackageRecord, Version, VersionWithSource, package::DistArchiveIdentifier,
    };
    use url::Url;

    use super::*;

    fn record(name: &str, version: &str, build: &str, build_number: u64) -> RepoDataRecord {
        let name = PackageName::from_str(name).unwrap();
        let version = VersionWithSource::from(Version::from_str(version).unwrap());
        let mut package_record = PackageRecord::new(name, version, build.to_owned());
        package_record.build_number = build_number;
        let file_name = format!(
            "{}-{}-{}.conda",
            package_record.name.as_normalized(),
            package_record.version,
            package_record.build
        );
        RepoDataRecord {
            url: Url::parse(&format!("https://example.com/noarch/{file_name}")).unwrap(),
            identifier: DistArchiveIdentifier::from_str(&file_name).unwrap(),
            channel: None,
            package_record,
        }
    }

    fn with_depends(mut record: RepoDataRecord, depends: &[&str]) -> RepoDataRecord {
        record.package_record.depends = depends.iter().map(|d| (*d).to_owned()).collect();
        record
    }

    fn with_timestamp(mut record: RepoDataRecord, timestamp_ms: i64) -> RepoDataRecord {
        record.package_record.timestamp = Some(
            jiff::Timestamp::from_millisecond(timestamp_ms)
                .unwrap()
                .into(),
        );
        record
    }

    fn with_track_features(mut record: RepoDataRecord, features: &[&str]) -> RepoDataRecord {
        record.package_record.track_features = features.iter().map(|f| (*f).to_owned()).collect();
        record
    }

    fn pythons() -> Vec<RepoDataRecord> {
        vec![
            record("python", "3.10.19", "h0", 0),
            record("python", "3.14.2", "h0", 0),
            record("python", "3.14.3", "h0", 0),
        ]
    }

    fn builds(records: &[&RepoDataRecord]) -> Vec<String> {
        records
            .iter()
            .map(|r| r.package_record.build.clone())
            .collect()
    }

    #[test]
    fn newer_python_variant_sorts_first_despite_older_timestamp() {
        let deps = pythons();
        let py310 = with_timestamp(
            with_depends(
                record("py-rattler", "0.25.0", "py310h2dd3e6a_5", 5),
                &["python >=3.10,<3.11.0a0", "python_abi 3.10.* *_cp310"],
            ),
            1_788_524_939_961,
        );
        let py314 = with_timestamp(
            with_depends(
                record("py-rattler", "0.25.0", "py314hbd51208_5", 5),
                &["python >=3.14,<3.15.0a0", "python_abi 3.14.* *_cp314"],
            ),
            1_788_524_937_259,
        );

        let mut records = vec![&py310, &py314];
        let mut index = DependencyIndex::new(deps.iter());
        sort_records(&mut records, &mut index);

        assert_eq!(builds(&records), ["py314hbd51208_5", "py310h2dd3e6a_5"]);
    }

    #[test]
    fn build_number_beats_dependency_versions() {
        let deps = pythons();
        let py314_4 = with_depends(
            record("py-rattler", "0.25.0", "py314h4bc2b79_4", 4),
            &["python >=3.14,<3.15.0a0"],
        );
        let py310_5 = with_depends(
            record("py-rattler", "0.25.0", "py310h2dd3e6a_5", 5),
            &["python >=3.10,<3.11.0a0"],
        );
        let py314_5 = with_depends(
            record("py-rattler", "0.25.0", "py314hbd51208_5", 5),
            &["python >=3.14,<3.15.0a0"],
        );
        let py310_4 = with_depends(
            record("py-rattler", "0.25.0", "py310hd89b7e0_4", 4),
            &["python >=3.10,<3.11.0a0"],
        );

        let mut records = vec![&py314_4, &py310_5, &py314_5, &py310_4];
        let mut index = DependencyIndex::new(deps.iter());
        sort_records(&mut records, &mut index);

        assert_eq!(
            builds(&records),
            [
                "py314hbd51208_5",
                "py310h2dd3e6a_5",
                "py314h4bc2b79_4",
                "py310hd89b7e0_4",
            ]
        );
    }

    #[test]
    fn falls_back_to_timestamp_when_dependencies_are_equal() {
        let deps = pythons();
        let older = with_timestamp(
            with_depends(
                record("foo", "1.0", "h1_0", 0),
                &["python >=3.14,<3.15.0a0"],
            ),
            1_000,
        );
        let newer = with_timestamp(
            with_depends(
                record("foo", "1.0", "h2_0", 0),
                &["python >=3.14,<3.15.0a0"],
            ),
            2_000,
        );

        let mut records = vec![&older, &newer];
        let mut index = DependencyIndex::new(deps.iter());
        sort_records(&mut records, &mut index);

        assert_eq!(builds(&records), ["h2_0", "h1_0"]);
    }

    #[test]
    fn falls_back_to_timestamp_without_dependency_index() {
        let older = with_timestamp(
            with_depends(
                record("foo", "1.0", "h1_0", 0),
                &["python >=3.10,<3.11.0a0"],
            ),
            1_000,
        );
        let newer = with_timestamp(
            with_depends(
                record("foo", "1.0", "h2_0", 0),
                &["python >=3.14,<3.15.0a0"],
            ),
            2_000,
        );

        let mut records = vec![&older, &newer];
        let mut index = DependencyIndex::default();
        sort_records(&mut records, &mut index);

        // Neither spec selects anything, so this is timestamp order.
        assert_eq!(builds(&records), ["h2_0", "h1_0"]);
    }

    #[test]
    fn spec_that_selects_a_version_beats_one_that_does_not() {
        let deps = pythons();
        let selects_nothing = with_timestamp(
            with_depends(record("foo", "1.0", "h1_0", 0), &["python >=3.20"]),
            2_000,
        );
        let selects_something = with_timestamp(
            with_depends(
                record("foo", "1.0", "h2_0", 0),
                &["python >=3.10,<3.11.0a0"],
            ),
            1_000,
        );

        let mut records = vec![&selects_nothing, &selects_something];
        let mut index = DependencyIndex::new(deps.iter());
        sort_records(&mut records, &mut index);

        assert_eq!(builds(&records), ["h2_0", "h1_0"]);
    }

    #[test]
    fn repeated_dependency_uses_most_restrictive_spec() {
        let deps = pythons();
        // A bare `python` next to a pin: only the pin should count, otherwise
        // both records would score the same.
        let py310 = with_timestamp(
            with_depends(
                record("foo", "1.0", "py310_0", 0),
                &["python", "python >=3.10,<3.11.0a0"],
            ),
            2_000,
        );
        let py314 = with_timestamp(
            with_depends(
                record("foo", "1.0", "py314_0", 0),
                &["python", "python >=3.14,<3.15.0a0"],
            ),
            1_000,
        );

        let mut records = vec![&py310, &py314];
        let mut index = DependencyIndex::new(deps.iter());
        sort_records(&mut records, &mut index);

        assert_eq!(builds(&records), ["py314_0", "py310_0"]);
    }

    #[test]
    fn only_shared_dependencies_are_compared() {
        let mut deps = pythons();
        deps.push(record("numpy", "2.0", "h0", 0));
        // `numpy` is only a dependency of one record, so it must not influence
        // the order; python decides.
        let py310_numpy = with_depends(
            record("foo", "1.0", "py310_0", 0),
            &["python >=3.10,<3.11.0a0", "numpy"],
        );
        let py314 = with_depends(
            record("foo", "1.0", "py314_0", 0),
            &["python >=3.14,<3.15.0a0"],
        );

        let mut records = vec![&py310_numpy, &py314];
        let mut index = DependencyIndex::new(deps.iter());
        sort_records(&mut records, &mut index);

        assert_eq!(builds(&records), ["py314_0", "py310_0"]);
    }

    #[test]
    fn name_version_and_track_features_order() {
        let tracked = with_track_features(record("b", "2.0", "tracked_0", 0), &["feat"]);
        let b_2 = record("b", "2.0", "plain_0", 0);
        let b_1 = record("b", "1.0", "plain_0", 0);
        let a = record("a", "9.0", "plain_0", 0);

        let mut records = vec![&tracked, &b_1, &b_2, &a];
        let mut index = DependencyIndex::default();
        sort_records(&mut records, &mut index);

        let names_and_builds: Vec<_> = records
            .iter()
            .map(|r| {
                format!(
                    "{}-{}-{}",
                    r.package_record.name.as_normalized(),
                    r.package_record.version,
                    r.package_record.build
                )
            })
            .collect();
        assert_eq!(
            names_and_builds,
            [
                "a-9.0-plain_0",
                "b-2.0-plain_0",
                "b-1.0-plain_0",
                "b-2.0-tracked_0"
            ]
        );
    }

    #[test]
    fn tiebreak_dependency_names_only_covers_tied_records() {
        let tied_a = with_depends(
            record("foo", "1.0", "py310_0", 0),
            &["python >=3.10,<3.11.0a0", "__osx >=11.0", "libfoo"],
        );
        let tied_b = with_depends(
            record("foo", "1.0", "py314_0", 0),
            &["python >=3.14,<3.15.0a0", "libbar"],
        );
        let alone = with_depends(record("foo", "2.0", "h0_0", 0), &["numpy"]);

        let mut names: Vec<_> = tiebreak_dependency_names(&[&alone, &tied_a, &tied_b])
            .into_iter()
            .map(|n| n.as_normalized().to_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["libbar", "libfoo", "python"]);
    }
}
