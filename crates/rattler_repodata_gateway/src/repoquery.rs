//! Post-processing queries over repodata records.
//!
//! The functions in this module are pure: they operate on records that have
//! already been fetched (e.g. through a `Gateway` query, or read from
//! `SparseRepoData`) and perform no I/O themselves.
//!
//! Currently this module provides [`who_needs`], a reverse dependency
//! lookup: given a package, find all records that reference it through
//! their `depends`, `constrains`, or run exports.

use std::sync::Arc;

use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, Matches, PackageName, PackageRecord, ParseMatchSpecOptions,
    RepoDataRecord,
};

/// The package to find reverse dependencies for with [`who_needs`].
///
/// The target determines how the dependency match specs of candidate
/// records are evaluated:
///
/// * [`Name`](Self::Name): any dependency naming the package matches,
///   regardless of its version or build constraints.
/// * [`Record`](Self::Record): a dependency matches if its match spec
///   matches the concrete record.
/// * [`VirtualPackage`](Self::VirtualPackage): a dependency matches if its
///   name, version, and build string constraints match the virtual
///   package.
#[derive(Debug, Clone)]
pub enum WhoNeedsTarget {
    /// Match every dependency that names this package.
    Name(PackageName),

    /// Match dependencies whose match spec matches this concrete record.
    Record(Box<PackageRecord>),

    /// Match dependencies whose match spec matches this virtual package
    /// (e.g. `__cuda`). Virtual packages have no records of their own, so
    /// they get their own variant instead of a full [`PackageRecord`].
    VirtualPackage(GenericVirtualPackage),
}

impl WhoNeedsTarget {
    /// The normalized name of the targeted package.
    fn name(&self) -> &str {
        match self {
            WhoNeedsTarget::Name(name) => name.as_normalized(),
            WhoNeedsTarget::Record(record) => record.name.as_normalized(),
            WhoNeedsTarget::VirtualPackage(virtual_package) => virtual_package.name.as_normalized(),
        }
    }

    /// Whether `spec` matches this target. Only called for dependencies
    /// that already passed the name check.
    fn matches(&self, spec: &MatchSpec) -> bool {
        match self {
            WhoNeedsTarget::Name(_) => true,
            WhoNeedsTarget::Record(record) => spec.matches(record.as_ref()),
            WhoNeedsTarget::VirtualPackage(virtual_package) => spec.matches(virtual_package),
        }
    }

    /// Whether the dependency string `dependency` references this target.
    /// `target_name` must be [`Self::name`], passed in so the caller can
    /// compute it once per scan instead of once per dependency.
    fn edge_matches(&self, target_name: &str, dependency: &str) -> bool {
        if PackageName::normalized_name_from_matchspec_str(dependency) != target_name {
            return false;
        }
        if matches!(self, WhoNeedsTarget::Name(_)) {
            return true;
        }
        match MatchSpec::from_str(dependency, ParseMatchSpecOptions::lenient()) {
            Ok(dependency_spec) => self.matches(&dependency_spec),
            // A dependency string that names the package but fails to parse
            // is reported rather than silently dropped.
            Err(_) => true,
        }
    }
}

impl From<PackageName> for WhoNeedsTarget {
    fn from(name: PackageName) -> Self {
        Self::Name(name)
    }
}

impl From<PackageRecord> for WhoNeedsTarget {
    fn from(record: PackageRecord) -> Self {
        Self::Record(Box::new(record))
    }
}

impl From<RepoDataRecord> for WhoNeedsTarget {
    fn from(record: RepoDataRecord) -> Self {
        record.package_record.into()
    }
}

impl From<GenericVirtualPackage> for WhoNeedsTarget {
    fn from(virtual_package: GenericVirtualPackage) -> Self {
        Self::VirtualPackage(virtual_package)
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
/// How dependencies are matched depends on the [`WhoNeedsTarget`] variant:
/// a [`PackageName`] reports every record with a dependency entry on that
/// name, while a concrete [`PackageRecord`] or [`GenericVirtualPackage`]
/// only reports dependents whose dependency match spec matches it. For
/// example, with a `python 3.13.1` record as the target, a record
/// depending on `python >=3.9` is reported while a record depending on
/// `python >=3.8,<3.10` is not.
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
    target: impl Into<WhoNeedsTarget>,
) -> Vec<Dependent<'r>> {
    let target = target.into();
    let target_name = target.name();

    let mut result = Vec::new();
    for record in records {
        for (dependencies, kind) in dependency_fields(record) {
            let Some(dependency) = dependencies
                .iter()
                .find(|dependency| target.edge_matches(target_name, dependency))
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

/// An owned counterpart of [`Dependent`], sharing the matching record
/// through an [`Arc`] instead of borrowing it. Returned by streaming scans
/// that discard the scanned records as they go (e.g. the gateway's
/// `who_needs` query) and therefore cannot hand out borrows.
#[derive(Debug, Clone)]
pub struct OwnedDependent {
    /// The record that references the queried package.
    pub record: Arc<RepoDataRecord>,

    /// The dependency string through which [`Self::record`] references the
    /// queried package.
    pub dependency: String,

    /// The field of the record that [`Self::dependency`] comes from.
    pub kind: DependencyKind,
}

/// The owned counterpart of [`who_needs`]: identical matching semantics,
/// but the matching records are shared via [`Arc`] so the caller can drop
/// `records` afterwards. See [`who_needs`] for the semantics.
pub fn who_needs_owned(
    records: &[Arc<RepoDataRecord>],
    target: &WhoNeedsTarget,
) -> Vec<OwnedDependent> {
    let target_name = target.name();

    let mut result = Vec::new();
    for record in records {
        for (dependencies, kind) in dependency_fields(record) {
            let Some(dependency) = dependencies
                .iter()
                .find(|dependency| target.edge_matches(target_name, dependency))
            else {
                continue;
            };
            result.push(OwnedDependent {
                record: record.clone(),
                dependency: dependency.clone(),
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
    pub fn who_needs(&self, target: impl Into<WhoNeedsTarget>) -> Vec<Dependent<'_>> {
        who_needs(self.repodata.iter().flat_map(crate::RepoData::iter), target)
    }
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{
        Version,
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

    fn name(name: &str) -> PackageName {
        name.parse().unwrap()
    }

    fn concrete(name: &str, version: &str, build: &str) -> PackageRecord {
        PackageRecord::new(
            name.parse().unwrap(),
            version.parse::<Version>().unwrap(),
            build.to_string(),
        )
    }

    fn names<'r>(dependents: &[Dependent<'r>]) -> Vec<&'r str> {
        dependents
            .iter()
            .map(|d| d.record.package_record.name.as_normalized())
            .collect()
    }

    #[test]
    fn test_by_name() {
        let records = test_records();
        let result = who_needs(&records, name("python"));
        assert_eq!(
            names(&result),
            vec!["old-lib", "numpy", "scipy", "pandas", "cpython-lib"]
        );
        assert_eq!(result[0].dependency, "python >=3.8,<3.10");
        assert_eq!(result[0].kind, DependencyKind::Depends);
    }

    #[test]
    fn test_by_record() {
        let records = test_records();
        // Only old-lib's constraint (>=3.8,<3.10) admits python 3.9.5; the
        // other dependents require >=3.10 or 3.13.*.
        let result = who_needs(&records, concrete("python", "3.9.5", "h123_0_cpython"));
        assert_eq!(names(&result), vec!["old-lib"]);

        // Everything except old-lib can use this python 3.13.1 build.
        let result = who_needs(&records, concrete("python", "3.13.1", "h123_0_cpython"));
        assert_eq!(
            names(&result),
            vec!["numpy", "scipy", "pandas", "cpython-lib"]
        );

        // cpython-lib requires build `*_cpython`, which excludes it for a
        // pypy build of python 3.13.
        let result = who_needs(&records, concrete("python", "3.13.1", "h123_0_pypy"));
        assert_eq!(names(&result), vec!["numpy", "scipy", "pandas"]);
    }

    #[test]
    fn test_constrains() {
        let records = test_records();
        let result = who_needs(&records, name("pandas"));
        assert_eq!(names(&result), vec!["pandas-stubs"]);
        assert_eq!(result[0].kind, DependencyKind::Constrains);
    }

    #[test]
    fn test_run_exports() {
        let records = test_records();
        // zlib references libzlib both through `depends` and through its
        // weak run exports; both edges are reported.
        let result = who_needs(&records, name("libzlib"));
        assert_eq!(names(&result), vec!["zlib", "zlib"]);
        assert_eq!(result[0].kind, DependencyKind::Depends);
        assert_eq!(result[0].dependency, "libzlib 1.3.1 h0_1");
        assert_eq!(
            result[1].kind,
            DependencyKind::RunExport(RunExportKind::Weak)
        );
        assert_eq!(result[1].dependency, "libzlib >=1.3.1,<2.0a0");

        // A record outside the run export range only matches the edges
        // whose constraint admits it.
        let result = who_needs(&records, concrete("libzlib", "2.1", "h0_0"));
        assert!(result.is_empty());

        let result = who_needs(&records, concrete("libzlib", "1.3.5", "h0_0"));
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
        let result = who_needs(&records, name("__cuda"));
        assert_eq!(names(&result), vec!["cuda-tool"]);

        // A concrete virtual package matches against the dependency's
        // version and build string constraints.
        let cuda = |version: &str| GenericVirtualPackage {
            name: "__cuda".parse().unwrap(),
            version: version.parse().unwrap(),
            build_string: "0".to_string(),
        };
        let result = who_needs(&records, cuda("12.4"));
        assert_eq!(names(&result), vec!["cuda-tool"]);
        let result = who_needs(&records, cuda("11.8"));
        assert!(result.is_empty());
    }
}
