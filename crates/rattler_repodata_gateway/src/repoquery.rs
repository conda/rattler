//! Post-processing queries over repodata records.
//!
//! The functions in this module are pure: they operate on records that have
//! already been fetched (e.g. through a `Gateway` query, or read from
//! `SparseRepoData`) and perform no I/O themselves.
//!
//! Currently this module provides [`who_needs`], a reverse dependency
//! lookup: given a package, find all records that reference it through
//! their `depends`, `constrains`, or run exports.

use rattler_conda_types::{MatchSpec, PackageName, ParseMatchSpecOptions, RepoDataRecord, Version};

/// The concrete package to find reverse dependencies for with [`who_needs`].
///
/// Unlike a `MatchSpec`, this describes a single concrete package — a name,
/// optionally pinned to a version and build string — so that the dependency
/// match specs of candidate records can be unambiguously matched against it.
#[derive(Debug, Clone)]
pub struct WhoNeedsTarget {
    /// The name of the package.
    pub name: PackageName,

    /// The version of the package. When `None`, matching is purely name
    /// based and version constraints on dependencies are ignored.
    pub version: Option<Version>,

    /// The build string of the package. When `None`, build string
    /// constraints on dependencies are ignored.
    pub build: Option<String>,
}

impl WhoNeedsTarget {
    /// Constructs a target that matches any version of `name`.
    pub fn new(name: PackageName) -> Self {
        Self {
            name,
            version: None,
            build: None,
        }
    }

    /// Restricts the target to a concrete version.
    #[must_use]
    pub fn with_version(self, version: Version) -> Self {
        Self {
            version: Some(version),
            ..self
        }
    }

    /// Restricts the target to a concrete build string.
    #[must_use]
    pub fn with_build(self, build: impl Into<String>) -> Self {
        Self {
            build: Some(build.into()),
            ..self
        }
    }
}

impl From<PackageName> for WhoNeedsTarget {
    fn from(name: PackageName) -> Self {
        Self::new(name)
    }
}

/// The run export field through which a record references the queried
/// package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunExportKind {
    /// The `weak` run exports (applied from host to run).
    Weak,
    /// The `strong` run exports (applied from build to host and run).
    Strong,
    /// The `noarch` run exports (applied only to noarch packages).
    Noarch,
    /// The `weak_constrains` run exports.
    WeakConstrains,
    /// The `strong_constrains` run exports.
    StrongConstrains,
}

/// How a dependent record references the queried package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependencyKind {
    /// The package is referenced through the `depends` field.
    Depends,
    /// The package is referenced through the `constrains` field.
    Constrains,
    /// The package is referenced through a run export: using the record as
    /// a build/host dependency injects a dependency on the queried package.
    RunExport(RunExportKind),
}

/// A record that references the package queried through [`who_needs`].
#[derive(Debug, Clone)]
pub struct Dependent<'r> {
    /// The record that references the queried package.
    pub record: &'r RepoDataRecord,

    /// The dependency string through which [`Self::record`] references the
    /// queried package.
    pub dependency: &'r str,

    /// The field of the record that [`Self::dependency`] comes from.
    pub kind: DependencyKind,
}

/// Returns all records in `records` that reference the package described by
/// `target` — its reverse dependencies.
///
/// A record references the target when one of its `depends` or `constrains`
/// entries, or one of its run exports, matches the target; the
/// [`Dependent::kind`] of each result tells which field matched, so callers
/// interested in only some of the kinds can simply filter the result. Note
/// that run exports can only be found on records that carry them (e.g.
/// repodata patched with run export information, or records enriched
/// through the gateway's run export extraction).
///
/// When the target carries nothing but a name, matching is purely name
/// based: every record with a dependency entry on that name is reported.
/// This also makes it possible to query for virtual packages (e.g.
/// `__cuda`) which have no records of their own.
///
/// When the target additionally carries a version (and optionally a build
/// string), a dependent is only reported if its dependency match spec
/// matches that concrete package. For example, with target
/// `python 3.13.1` a record depending on `python >=3.9` is reported while
/// a record depending on `python >=3.8,<3.10` is not.
///
/// Note that reverse dependency lookup requires the *complete* set of
/// records of the queried channels and platforms — any record not passed in
/// here is invisible to the search. Use a wildcard gateway query (spec `*`)
/// or `SparseRepoData` to obtain them.
///
/// The result keeps the order of `records`. Every record is reported at
/// most once per [`DependencyKind`], through the first dependency string of
/// that field that matches.
pub fn who_needs<'r>(
    records: impl IntoIterator<Item = &'r RepoDataRecord>,
    target: &WhoNeedsTarget,
) -> Vec<Dependent<'r>> {
    let target_name = target.name.as_normalized();
    let is_pinned = target.version.is_some() || target.build.is_some();

    let edge_matches = |dependency: &str| -> bool {
        if PackageName::normalized_name_from_matchspec_str(dependency) != target_name {
            return false;
        }
        if !is_pinned {
            return true;
        }
        match MatchSpec::from_str(dependency, ParseMatchSpecOptions::lenient()) {
            Ok(dependency_spec) => {
                let version_matches = match (&target.version, &dependency_spec.version) {
                    (Some(version), Some(constraint)) => constraint.matches(version),
                    _ => true,
                };
                let build_matches = match (&target.build, &dependency_spec.build) {
                    (Some(build), Some(matcher)) => matcher.matches(build),
                    _ => true,
                };
                version_matches && build_matches
            }
            // A dependency string that names the package but fails to parse
            // is reported rather than silently dropped.
            Err(_) => true,
        }
    };

    let mut result = Vec::new();
    for record in records {
        for (dependencies, kind) in dependency_fields(record) {
            let Some(dependency) = dependencies
                .iter()
                .find(|dependency| edge_matches(dependency))
            else {
                continue;
            };
            result.push(Dependent {
                record,
                dependency,
                kind,
            });
        }
    }
    result
}

/// Returns the dependency fields of `record` to consider.
fn dependency_fields(record: &RepoDataRecord) -> [(&[String], DependencyKind); 7] {
    const NO_DEPS: &[String] = &[];
    let run_exports = record.package_record.run_exports.as_ref();
    let run_export_field =
        |field: fn(&rattler_conda_types::package::RunExportsJson) -> &Vec<String>| {
            run_exports.map_or(NO_DEPS, |run_exports| field(run_exports).as_slice())
        };
    [
        (
            record.package_record.depends.as_slice(),
            DependencyKind::Depends,
        ),
        (
            record.package_record.constrains.as_slice(),
            DependencyKind::Constrains,
        ),
        (
            run_export_field(|re| &re.weak),
            DependencyKind::RunExport(RunExportKind::Weak),
        ),
        (
            run_export_field(|re| &re.strong),
            DependencyKind::RunExport(RunExportKind::Strong),
        ),
        (
            run_export_field(|re| &re.noarch),
            DependencyKind::RunExport(RunExportKind::Noarch),
        ),
        (
            run_export_field(|re| &re.weak_constrains),
            DependencyKind::RunExport(RunExportKind::WeakConstrains),
        ),
        (
            run_export_field(|re| &re.strong_constrains),
            DependencyKind::RunExport(RunExportKind::StrongConstrains),
        ),
    ]
}

#[cfg(feature = "gateway")]
impl crate::RepoDataQueryOutput {
    /// Returns all records in this query result that reference the package
    /// described by `target`. See [`who_needs`] for the exact semantics.
    ///
    /// Reverse dependency lookup requires the complete set of records of
    /// the queried channels and platforms, so the query this output came
    /// from should have used a wildcard spec (`*`).
    pub fn who_needs(&self, target: &WhoNeedsTarget) -> Vec<Dependent<'_>> {
        who_needs(self.repodata.iter().flat_map(crate::RepoData::iter), target)
    }
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{
        PackageRecord,
        package::{DistArchiveIdentifier, RunExportsJson},
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
            version.parse::<Version>().unwrap(),
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
        let mut zlib = record("zlib", "1.3.1", "0", vec!["libzlib 1.3.1 h0_1"], vec![]);
        zlib.package_record.run_exports = Some(RunExportsJson {
            weak: vec!["libzlib >=1.3.1,<2.0a0".to_string()],
            ..RunExportsJson::default()
        });
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
            record(
                "cpython-lib",
                "1.0.0",
                "0",
                vec!["python 3.13.* *_cpython"],
                vec![],
            ),
            zlib,
        ]
    }

    fn target(name: &str) -> WhoNeedsTarget {
        WhoNeedsTarget::new(name.parse().unwrap())
    }

    fn version(v: &str) -> Version {
        v.parse().unwrap()
    }

    fn names<'r>(dependents: &[Dependent<'r>]) -> Vec<&'r str> {
        dependents
            .iter()
            .map(|d| d.record.package_record.name.as_normalized())
            .collect()
    }

    #[test]
    fn test_direct_by_name() {
        let records = test_records();
        let result = who_needs(&records, &target("python"));
        assert_eq!(
            names(&result),
            vec!["old-lib", "numpy", "scipy", "pandas", "cpython-lib"]
        );
        assert_eq!(result[0].dependency, "python >=3.8,<3.10");
        assert_eq!(result[0].kind, DependencyKind::Depends);
    }

    #[test]
    fn test_pinned_version() {
        let records = test_records();
        // Only old-lib's constraint (>=3.8,<3.10) admits python 3.9.5; the
        // other dependents require >=3.10 or 3.13.*.
        let result = who_needs(&records, &target("python").with_version(version("3.9.5")));
        assert_eq!(names(&result), vec!["old-lib"]);

        // Everything except old-lib can use python 3.13.1.
        let result = who_needs(&records, &target("python").with_version(version("3.13.1")));
        assert_eq!(
            names(&result),
            vec!["numpy", "scipy", "pandas", "cpython-lib"]
        );
    }

    #[test]
    fn test_pinned_build() {
        let records = test_records();
        // cpython-lib requires build `*_cpython`, which excludes it for a
        // pypy build of python 3.13.
        let result = who_needs(
            &records,
            &target("python")
                .with_version(version("3.13.1"))
                .with_build("h123_0_pypy"),
        );
        assert_eq!(names(&result), vec!["numpy", "scipy", "pandas"]);

        let result = who_needs(
            &records,
            &target("python")
                .with_version(version("3.13.1"))
                .with_build("h123_0_cpython"),
        );
        assert_eq!(
            names(&result),
            vec!["numpy", "scipy", "pandas", "cpython-lib"]
        );
    }

    #[test]
    fn test_constrains() {
        let records = test_records();
        let result = who_needs(&records, &target("pandas"));
        assert_eq!(names(&result), vec!["pandas-stubs"]);
        assert_eq!(result[0].kind, DependencyKind::Constrains);
    }

    #[test]
    fn test_run_exports() {
        let records = test_records();
        // zlib references libzlib both through `depends` and through its
        // weak run exports; both edges are reported.
        let result = who_needs(&records, &target("libzlib"));
        assert_eq!(names(&result), vec!["zlib", "zlib"]);
        assert_eq!(result[0].kind, DependencyKind::Depends);
        assert_eq!(result[0].dependency, "libzlib 1.3.1 h0_1");
        assert_eq!(
            result[1].kind,
            DependencyKind::RunExport(RunExportKind::Weak)
        );
        assert_eq!(result[1].dependency, "libzlib >=1.3.1,<2.0a0");

        // A pinned version outside the run export range only matches the
        // edges whose constraint admits it.
        let result = who_needs(&records, &target("libzlib").with_version(version("2.1")));
        assert!(result.is_empty());

        let result = who_needs(&records, &target("libzlib").with_version(version("1.3.5")));
        assert_eq!(names(&result), vec!["zlib"]);
        assert_eq!(
            result[0].kind,
            DependencyKind::RunExport(RunExportKind::Weak)
        );
    }

    #[test]
    fn test_virtual_package() {
        let records = test_records();
        // No record named __cuda exists; name-based matching still works.
        let result = who_needs(&records, &target("__cuda"));
        assert_eq!(names(&result), vec!["cuda-tool"]);

        // And so does matching a concrete virtual package version.
        let result = who_needs(&records, &target("__cuda").with_version(version("11.8")));
        assert!(result.is_empty());
    }
}
