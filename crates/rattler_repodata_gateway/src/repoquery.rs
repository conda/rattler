//! Post-processing queries over repodata records.
//!
//! The functions in this module are pure: they operate on records that have
//! already been fetched (e.g. through a `Gateway` query, or read from
//! `SparseRepoData`) and perform no I/O themselves.
//!
//! Currently this module provides [`who_needs`], the equivalent of
//! `conda repoquery whoneeds`: given a package (or match spec), find all
//! records that reference it through their `depends` (and optionally
//! `constrains`) entries.

use std::collections::{HashMap, HashSet};

use rattler_conda_types::{MatchSpec, Matches, PackageName, ParseMatchSpecOptions, RepoDataRecord};

/// How a dependent record references the queried package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependencyKind {
    /// The package is referenced through the `depends` field.
    Depends,
    /// The package is referenced through the `constrains` field.
    Constrains,
}

/// A record that (transitively) references the package queried through
/// [`who_needs`].
#[derive(Debug, Clone)]
pub struct Dependent<'r> {
    /// The record that references the queried package.
    pub record: &'r RepoDataRecord,

    /// The dependency string through which [`Self::record`] references the
    /// queried package (for a direct dependent) or the previous level of
    /// dependents (for a transitive one).
    pub dependency: &'r str,

    /// Whether [`Self::dependency`] comes from the `depends` or the
    /// `constrains` field of the record.
    pub kind: DependencyKind,

    /// The distance from the queried package: `1` for a record that
    /// references the package directly, `2` for a record that references a
    /// direct dependent, and so on. Always `1` unless
    /// [`WhoNeedsOptions::recursive`] is enabled. When a record is reachable
    /// at multiple depths, the smallest depth is reported.
    pub depth: usize,
}

/// Options that control the behavior of [`who_needs`].
#[derive(Debug, Clone, Default)]
pub struct WhoNeedsOptions {
    /// Also report records that reference the queried package through their
    /// `constrains` field. Disabled by default.
    pub include_constrains: bool,

    /// Also report records that reference the queried package transitively:
    /// packages that depend on a package that (transitively) depends on the
    /// queried package. Disabled by default.
    pub recursive: bool,
}

/// An error returned by [`who_needs`].
#[derive(Debug, thiserror::Error)]
pub enum WhoNeedsError {
    /// The queried match spec does not have an exact package name (e.g. a
    /// glob or regex name), which reverse dependency lookup requires.
    #[error("the match spec '{0}' does not contain an exact package name")]
    MatchSpecWithoutExactName(Box<MatchSpec>),
}

/// Returns all records in `records` that reference the package described by
/// `spec` — the equivalent of `conda repoquery whoneeds`.
///
/// The `spec` must contain an exact package name. When it carries nothing
/// but a name, matching is purely name based: every record with a `depends`
/// entry (or `constrains` entry, when
/// [`WhoNeedsOptions::include_constrains`] is enabled) on that name is
/// reported. This also makes it possible to query for virtual packages
/// (e.g. `__cuda`) which have no records of their own.
///
/// When the spec additionally constrains the version, build, or build
/// number (e.g. `python >=3.13`), a dependent is only reported if its
/// dependency constraint admits at least one record in `records` that
/// matches `spec`. In other words: "who could use a python that matches
/// `>=3.13`".
///
/// Note that reverse dependency lookup requires the *complete* set of
/// records of the queried channels and platforms — any record not passed in
/// here is invisible to the search. Use a wildcard gateway query (spec `*`)
/// or `SparseRepoData` to obtain them.
///
/// The result is ordered by ascending [`Dependent::depth`]; within a depth
/// records keep the order of `records`. Every record is reported at most
/// once, through the first dependency string that matches (`depends`
/// entries take precedence over `constrains` entries).
pub fn who_needs<'r>(
    records: impl IntoIterator<Item = &'r RepoDataRecord>,
    spec: &MatchSpec,
    options: &WhoNeedsOptions,
) -> Result<Vec<Dependent<'r>>, WhoNeedsError> {
    let target_name = spec
        .name
        .clone()
        .into_exact()
        .ok_or_else(|| WhoNeedsError::MatchSpecWithoutExactName(Box::new(spec.clone())))?;
    let target_name = target_name.as_normalized();

    let records: Vec<&'r RepoDataRecord> = records.into_iter().collect();

    // When the spec constrains more than the name, collect the records that
    // match it so dependency constraints can be checked against them.
    let is_constrained =
        spec.version.is_some() || spec.build.is_some() || spec.build_number.is_some();
    let targets: Vec<&RepoDataRecord> = if is_constrained {
        records
            .iter()
            .copied()
            .filter(|record| spec.matches(*record))
            .collect()
    } else {
        Vec::new()
    };

    let edge_matches = |dependency: &str| -> bool {
        if PackageName::normalized_name_from_matchspec_str(dependency) != target_name {
            return false;
        }
        if !is_constrained {
            return true;
        }
        match MatchSpec::from_str(dependency, ParseMatchSpecOptions::lenient()) {
            Ok(dependency_spec) => targets
                .iter()
                .any(|target| dependency_spec.matches(&target.package_record)),
            // A dependency string that names the package but fails to parse
            // is reported rather than silently dropped.
            Err(_) => true,
        }
    };

    // Direct dependents.
    let mut result = Vec::new();
    let mut emitted: HashSet<usize> = HashSet::new();
    let mut frontier: Vec<&str> = Vec::new();
    let mut visited_names: HashSet<&str> = HashSet::from([target_name]);
    for (idx, record) in records.iter().enumerate() {
        let Some((dependency, kind)) = first_matching_edge(record, options, &edge_matches) else {
            continue;
        };
        emitted.insert(idx);
        let name = record.package_record.name.as_normalized();
        if visited_names.insert(name) {
            frontier.push(name);
        }
        result.push(Dependent {
            record,
            dependency,
            kind,
            depth: 1,
        });
    }

    if !options.recursive || result.is_empty() {
        return Ok(result);
    }

    // Transitive dependents: breadth-first search over a reverse dependency
    // index, so every record and edge is visited at most once regardless of
    // the number of levels.
    let mut reverse_index: HashMap<String, Vec<(usize, &'r str, DependencyKind)>> = HashMap::new();
    for (idx, record) in records.iter().enumerate() {
        for (dependencies, kind) in dependency_fields(record, options) {
            for dependency in dependencies {
                let name = PackageName::normalized_name_from_matchspec_str(dependency);
                // Allocates only once per unique dependency name.
                match reverse_index.get_mut(name.as_ref()) {
                    Some(edges) => edges.push((idx, dependency, kind)),
                    None => {
                        reverse_index.insert(name.into_owned(), vec![(idx, dependency, kind)]);
                    }
                }
            }
        }
    }

    let mut depth = 1;
    while !frontier.is_empty() {
        depth += 1;
        // Sort for deterministic traversal order within a depth.
        frontier.sort_unstable();
        let mut next_frontier = Vec::new();
        for name in frontier {
            let Some(edges) = reverse_index.get(name) else {
                continue;
            };
            for &(idx, dependency, kind) in edges {
                if !emitted.insert(idx) {
                    continue;
                }
                let record = records[idx];
                let record_name = record.package_record.name.as_normalized();
                if visited_names.insert(record_name) {
                    next_frontier.push(record_name);
                }
                result.push(Dependent {
                    record,
                    dependency,
                    kind,
                    depth,
                });
            }
        }
        frontier = next_frontier;
    }

    Ok(result)
}

/// Returns the dependency fields of `record` to consider, in match
/// precedence order.
fn dependency_fields<'r>(
    record: &'r RepoDataRecord,
    options: &WhoNeedsOptions,
) -> impl Iterator<Item = (&'r [String], DependencyKind)> {
    std::iter::once((
        record.package_record.depends.as_slice(),
        DependencyKind::Depends,
    ))
    .chain(options.include_constrains.then_some((
        record.package_record.constrains.as_slice(),
        DependencyKind::Constrains,
    )))
}

/// Returns the first dependency string of `record` for which `edge_matches`
/// returns true, together with the field it came from.
fn first_matching_edge<'r>(
    record: &'r RepoDataRecord,
    options: &WhoNeedsOptions,
    edge_matches: &impl Fn(&str) -> bool,
) -> Option<(&'r str, DependencyKind)> {
    dependency_fields(record, options)
        .flat_map(|(dependencies, kind)| dependencies.iter().map(move |dep| (dep.as_str(), kind)))
        .find(|(dependency, _)| edge_matches(dependency))
}

#[cfg(feature = "gateway")]
impl crate::RepoDataQueryOutput {
    /// Returns all records in this query result that reference the package
    /// described by `spec`. See [`who_needs`] for the exact semantics.
    ///
    /// Reverse dependency lookup requires the complete set of records of
    /// the queried channels and platforms, so the query this output came
    /// from should have used a wildcard spec (`*`).
    pub fn who_needs(
        &self,
        spec: &MatchSpec,
        options: &WhoNeedsOptions,
    ) -> Result<Vec<Dependent<'_>>, WhoNeedsError> {
        who_needs(
            self.repodata.iter().flat_map(crate::RepoData::iter),
            spec,
            options,
        )
    }
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{
        PackageRecord, ParseMatchSpecOptions, package::DistArchiveIdentifier,
    };
    use url::Url;

    use super::*;

    fn record(
        name: &str,
        version: &str,
        build: &str,
        depends: Vec<&str>,
        constrains: Vec<&str>,
    ) -> RepoDataRecord {
        let mut package_record = PackageRecord::new(
            name.parse().unwrap(),
            version.parse::<rattler_conda_types::Version>().unwrap(),
            build.to_string(),
        );
        package_record.depends = depends.into_iter().map(String::from).collect();
        package_record.constrains = constrains.into_iter().map(String::from).collect();
        let file_name = format!("{name}-{version}-{build}.conda");
        RepoDataRecord {
            package_record,
            identifier: DistArchiveIdentifier::try_from_filename(&file_name).unwrap(),
            url: Url::parse(&format!("https://example.com/{file_name}")).unwrap(),
            channel: Some(String::from("test-channel")),
        }
    }

    fn test_records() -> Vec<RepoDataRecord> {
        vec![
            record("python", "3.9.0", "0", vec![], vec![]),
            record("python", "3.13.0", "0", vec![], vec![]),
            record("old-lib", "1.0.0", "0", vec!["python >=3.8,<3.10"], vec![]),
            record("numpy", "2.1.0", "0", vec!["python >=3.10"], vec![]),
            record(
                "scipy",
                "1.14.0",
                "0",
                vec!["numpy >=2.0", "python >=3.10"],
                vec![],
            ),
            record("pandas-stubs", "2.2.0", "0", vec![], vec!["pandas >=2.2"]),
            record(
                "pandas",
                "2.2.0",
                "0",
                vec!["numpy >=2.0", "python >=3.10"],
                vec![],
            ),
            record("cuda-tool", "1.0.0", "0", vec!["__cuda >=12"], vec![]),
        ]
    }

    fn spec(s: &str) -> MatchSpec {
        MatchSpec::from_str(s, ParseMatchSpecOptions::strict()).unwrap()
    }

    fn names<'r>(dependents: &[Dependent<'r>]) -> Vec<(&'r str, usize)> {
        dependents
            .iter()
            .map(|d| (d.record.package_record.name.as_normalized(), d.depth))
            .collect()
    }

    #[test]
    fn test_direct_by_name() {
        let records = test_records();
        let result = who_needs(&records, &spec("python"), &WhoNeedsOptions::default()).unwrap();
        assert_eq!(
            names(&result),
            vec![("old-lib", 1), ("numpy", 1), ("scipy", 1), ("pandas", 1)]
        );
        assert_eq!(result[0].dependency, "python >=3.8,<3.10");
        assert_eq!(result[0].kind, DependencyKind::Depends);
    }

    #[test]
    fn test_version_constrained() {
        let records = test_records();
        // old-lib requires python <3.10 and therefore cannot use python 3.13.
        let result = who_needs(
            &records,
            &spec("python >=3.13"),
            &WhoNeedsOptions::default(),
        )
        .unwrap();
        assert_eq!(
            names(&result),
            vec![("numpy", 1), ("scipy", 1), ("pandas", 1)]
        );
    }

    #[test]
    fn test_version_constrained_without_matching_target() {
        let records = test_records();
        // No python 4 exists in the records, so nothing can use it.
        let result = who_needs(&records, &spec("python >=4"), &WhoNeedsOptions::default()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_constrains() {
        let records = test_records();
        let result = who_needs(&records, &spec("pandas"), &WhoNeedsOptions::default()).unwrap();
        assert!(result.is_empty());

        let result = who_needs(
            &records,
            &spec("pandas"),
            &WhoNeedsOptions {
                include_constrains: true,
                ..WhoNeedsOptions::default()
            },
        )
        .unwrap();
        assert_eq!(names(&result), vec![("pandas-stubs", 1)]);
        assert_eq!(result[0].kind, DependencyKind::Constrains);
    }

    #[test]
    fn test_recursive() {
        let records = test_records();
        let result = who_needs(
            &records,
            &spec("numpy"),
            &WhoNeedsOptions {
                recursive: true,
                ..WhoNeedsOptions::default()
            },
        )
        .unwrap();
        // scipy and pandas depend on numpy directly; pandas-stubs only
        // reaches numpy through a `constrains` edge, which is disabled.
        assert_eq!(names(&result), vec![("scipy", 1), ("pandas", 1)]);

        let result = who_needs(
            &records,
            &spec("numpy"),
            &WhoNeedsOptions {
                recursive: true,
                include_constrains: true,
            },
        )
        .unwrap();
        assert_eq!(
            names(&result),
            vec![("scipy", 1), ("pandas", 1), ("pandas-stubs", 2)]
        );
        assert_eq!(result[2].dependency, "pandas >=2.2");
        assert_eq!(result[2].kind, DependencyKind::Constrains);
    }

    #[test]
    fn test_virtual_package() {
        let records = test_records();
        // No record named __cuda exists; name-based matching still works.
        let result = who_needs(&records, &spec("__cuda"), &WhoNeedsOptions::default()).unwrap();
        assert_eq!(names(&result), vec![("cuda-tool", 1)]);
    }

    #[test]
    fn test_requires_exact_name() {
        let records = test_records();
        let glob_spec = MatchSpec::from_str(
            "py*",
            ParseMatchSpecOptions::strict().with_exact_names_only(false),
        )
        .unwrap();
        let result = who_needs(&records, &glob_spec, &WhoNeedsOptions::default());
        assert!(matches!(
            result,
            Err(WhoNeedsError::MatchSpecWithoutExactName(_))
        ));
    }
}
