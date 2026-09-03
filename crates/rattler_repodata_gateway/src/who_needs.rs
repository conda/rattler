//! The input and output types of the reverse dependency lookup performed by
//! [`Gateway::who_needs`](crate::Gateway::who_needs), plus the pure matcher
//! it is built on.
//!
//! A record is a reverse dependency of a package when it references that
//! package through its `depends`, `constrains`, `extra_depends`, or run
//! exports.
//!
//! This module answers *what* counts as a reverse dependency and is free of
//! I/O; the gateway's `who_needs_query` module answers *where* the records
//! come from.

use std::{
    fmt::{self, Display, Formatter},
    sync::Arc,
};

use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, Matches, PackageName, PackageRecord, ParseMatchSpecOptions,
    RepoDataRecord, package::RunExportsJson,
};

/// The package to find reverse dependencies for with
/// [`Gateway::who_needs`](crate::Gateway::who_needs).
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
    /// Whether the dependency match spec `spec` references this target.
    ///
    /// Every variant checks the name: a [`Name`](Self::Name) target checks
    /// only that, while the concrete variants additionally require the
    /// version and build constraints of `spec` to hold. The name of `spec`
    /// is always an exact [`PackageName`], because dependencies are parsed
    /// with `exact_names_only` — a glob or regex name is a parse error, not
    /// a spec that could match several packages.
    fn matches(&self, spec: &MatchSpec) -> bool {
        match self {
            WhoNeedsTarget::Name(name) => spec.name.matches(name),
            WhoNeedsTarget::Record(record) => spec.matches(record.as_ref()),
            WhoNeedsTarget::VirtualPackage(virtual_package) => spec.matches(virtual_package),
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DependencyKind {
    /// The package is referenced through the `depends` field.
    Depends,
    /// The package is referenced through the `constrains` field.
    Constrains,
    /// The package is referenced through the dependencies of an optional
    /// feature in `extra_depends`. The payload is the name of the extra, so
    /// the reference only applies when that extra is enabled.
    ExtraDepends(String),
    /// The package is referenced through a run export: using the record as
    /// a build/host dependency injects a dependency on the queried package.
    RunExport(RunExportKind),
}

impl Display for DependencyKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            DependencyKind::Depends => write!(f, "depends"),
            DependencyKind::Constrains => write!(f, "constrains"),
            DependencyKind::ExtraDepends(extra) => write!(f, "extra_depends[{extra}]"),
            DependencyKind::RunExport(kind) => write!(f, "run_exports/{kind}"),
        }
    }
}

impl Display for RunExportKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let name = match self {
            RunExportKind::Weak => "weak",
            RunExportKind::Strong => "strong",
            RunExportKind::Noarch => "noarch",
            RunExportKind::WeakConstrains => "weak_constrains",
            RunExportKind::StrongConstrains => "strong_constrains",
        };
        write!(f, "{name}")
    }
}

/// A record that references the package queried through
/// [`Gateway::who_needs`](crate::Gateway::who_needs).
///
/// The record is shared through an [`Arc`] because the scan discards the
/// records it visits as it goes and therefore cannot hand out borrows into
/// them.
#[derive(Debug, Clone)]
pub struct Dependent {
    /// The record that references the queried package.
    pub record: Arc<RepoDataRecord>,

    /// The dependency string through which [`Self::record`] references the
    /// queried package.
    pub dependency: String,

    /// The field of the record that [`Self::dependency`] comes from.
    pub kind: DependencyKind,
}

/// Returns all of `records` that reference the package described by `target`
/// — its reverse dependencies.
///
/// A record references the target when one of its `depends`, `constrains`,
/// or `extra_depends` entries, or one of its run exports, matches the
/// target; the [`Dependent::kind`] of each result tells which field
/// matched, so callers interested in only some of the kinds can simply
/// filter the result. Note that run exports can only be found on records
/// that carry them (e.g. repodata patched with run export information, or
/// records enriched through the gateway's run export extraction).
///
/// How dependencies are matched depends on the [`WhoNeedsTarget`] variant:
/// a [`PackageName`] reports every record with a dependency entry on that
/// name, while a concrete [`PackageRecord`] or [`GenericVirtualPackage`]
/// only reports dependents whose dependency match spec matches it. For
/// example, with a `python 3.13.1` record as the target, a record
/// depending on `python >=3.9` is reported while a record depending on
/// `python >=3.8,<3.10` is not.
///
/// The result keeps the order of `records`. Every record is reported at
/// most once per [`DependencyKind`], through the first dependency string of
/// that field that matches — so a record referencing the target from two
/// different extras yields one result per extra.
///
/// A dependency string that is not a valid match spec is logged at warn
/// level and treated as a non-match. Published repodata does contain such
/// entries (unrendered jinja like `pin_compatible('xtensor')` occurs on
/// conda-forge), and a scan covers a whole channel, so one corrupt record
/// must not fail the entire query.
///
/// This is the per-batch core of the channel-wide scan performed by
/// [`Gateway::who_needs`](crate::Gateway::who_needs). It is deliberately
/// not public: a reverse dependency lookup is only correct over the
/// *complete* set of records of the queried channels and platforms, and
/// assembling those is exactly what the gateway query does.
pub(crate) fn who_needs(
    records: &[Arc<RepoDataRecord>],
    target: &WhoNeedsTarget,
) -> Vec<Dependent> {
    let mut result = Vec::new();
    for record in records {
        matching_edges(record, target, |dependency, kind| {
            result.push(Dependent {
                record: record.clone(),
                dependency: dependency.to_string(),
                kind,
            });
        });
    }
    result
}

/// Invokes `on_match` once per dependency field of `record` that
/// references `target`, with the first matching dependency string of that
/// field and the kind describing where it came from.
fn matching_edges<'r>(
    record: &'r RepoDataRecord,
    target: &WhoNeedsTarget,
    mut on_match: impl FnMut(&'r str, DependencyKind),
) {
    let package_record = &record.package_record;
    // Repodata carries the full matchspec syntax surface, including the
    // extras, conditional (`pkg[when="__linux"]`) and flag forms that
    // `lenient()` leaves off by default. Names must stay exact, though:
    // `exact_names_only` keeps a glob or regex name a parse error rather
    // than a spec that silently matches many packages.
    let options = ParseMatchSpecOptions::lenient()
        .with_extras(true)
        .with_conditionals(true)
        .with_flags(true);
    let mut first_match = |dependencies: &'r [String], kind: DependencyKind| {
        for dependency in dependencies {
            let spec = match MatchSpec::from_str(dependency, options) {
                Ok(spec) => spec,
                // Published repodata does contain unparseable specs. Warn
                // and move on: the entry cannot be shown to reference the
                // target, and one bad record must not fail a whole-channel
                // scan.
                Err(error) => {
                    tracing::warn!(
                        "ignoring the {kind} entry '{dependency}' of '{}', which is not a valid match spec: {error}",
                        record.identifier,
                    );
                    continue;
                }
            };
            if target.matches(&spec) {
                on_match(dependency, kind);
                return;
            }
        }
    };

    first_match(&package_record.depends, DependencyKind::Depends);
    first_match(&package_record.constrains, DependencyKind::Constrains);
    for (extra, dependencies) in &package_record.extra_depends {
        first_match(dependencies, DependencyKind::ExtraDepends(extra.clone()));
    }
    if let Some(run_exports) = package_record.run_exports.as_ref() {
        for (field, kind) in run_export_fields(run_exports) {
            first_match(field, DependencyKind::RunExport(kind));
        }
    }
}

/// The run export fields of `run_exports` paired with the kind that
/// identifies them.
fn run_export_fields(run_exports: &RunExportsJson) -> [(&[String], RunExportKind); 5] {
    [
        (run_exports.weak.as_slice(), RunExportKind::Weak),
        (run_exports.strong.as_slice(), RunExportKind::Strong),
        (run_exports.noarch.as_slice(), RunExportKind::Noarch),
        (
            run_exports.weak_constrains.as_slice(),
            RunExportKind::WeakConstrains,
        ),
        (
            run_exports.strong_constrains.as_slice(),
            RunExportKind::StrongConstrains,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{
        Version,
        package::{DistArchiveIdentifier, RunExportsJson},
    };
    use url::Url;

    use super::*;

    /// Runs [`who_needs`] over `records`, converting `target` for brevity.
    fn dependents(
        records: &[Arc<RepoDataRecord>],
        target: impl Into<WhoNeedsTarget>,
    ) -> Vec<Dependent> {
        who_needs(records, &target.into())
    }

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

    fn test_records() -> Vec<Arc<RepoDataRecord>> {
        let mut zlib = record("zlib", "1.3.1", "0", vec!["libzlib 1.3.1 h0_1"], vec![]);
        zlib.package_record.run_exports = Some(RunExportsJson {
            weak: vec!["libzlib >=1.3.1,<2.0a0".to_string()],
            ..RunExportsJson::default()
        });
        // References pandas only through optional features, and from two
        // different extras with disjoint constraints.
        let mut extras_tool = record("extras-tool", "1.0.0", "0", vec![], vec![]);
        extras_tool.package_record.extra_depends = [
            ("plot".to_string(), vec!["pandas >=2.2".to_string()]),
            ("legacy".to_string(), vec!["pandas <2.0".to_string()]),
        ]
        .into_iter()
        .collect();
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
            extras_tool,
        ]
        .into_iter()
        .map(Arc::new)
        .collect()
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

    fn names(dependents: &[Dependent]) -> Vec<&str> {
        dependents
            .iter()
            .map(|d| d.record.package_record.name.as_normalized())
            .collect()
    }

    #[test]
    fn test_by_name() {
        let records = test_records();
        let result = dependents(&records, name("python"));
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
        let result = dependents(&records, concrete("python", "3.9.5", "h123_0_cpython"));
        assert_eq!(names(&result), vec!["old-lib"]);

        // Everything except old-lib can use this python 3.13.1 build.
        let result = dependents(&records, concrete("python", "3.13.1", "h123_0_cpython"));
        assert_eq!(
            names(&result),
            vec!["numpy", "scipy", "pandas", "cpython-lib"]
        );

        // cpython-lib requires build `*_cpython`, which excludes it for a
        // pypy build of python 3.13.
        let result = dependents(&records, concrete("python", "3.13.1", "h123_0_pypy"));
        assert_eq!(names(&result), vec!["numpy", "scipy", "pandas"]);
    }

    #[test]
    fn test_constrains() {
        let records = test_records();
        let result = dependents(&records, name("pandas"));
        // extras-tool appears twice: it names pandas from two extras, see
        // `test_extra_depends`.
        assert_eq!(
            names(&result),
            vec!["pandas-stubs", "extras-tool", "extras-tool"]
        );
        assert_eq!(result[0].kind, DependencyKind::Constrains);
    }

    #[test]
    fn test_extra_depends() {
        let records = test_records();
        // extras-tool only references pandas from its optional features, so
        // it is invisible unless `extra_depends` is scanned. Both extras
        // name pandas, so both edges are reported (extras are visited in
        // the record's sorted `extra_depends` order).
        let result = dependents(&records, name("pandas"));
        let extras: Vec<_> = result
            .iter()
            .filter(|d| d.record.package_record.name.as_normalized() == "extras-tool")
            .map(|d| (d.kind.clone(), d.dependency.as_str()))
            .collect();
        assert_eq!(
            extras,
            vec![
                (
                    DependencyKind::ExtraDepends("legacy".to_string()),
                    "pandas <2.0"
                ),
                (
                    DependencyKind::ExtraDepends("plot".to_string()),
                    "pandas >=2.2"
                ),
            ]
        );

        // A concrete target only matches the extras whose constraint admits
        // it: pandas 2.2.0 satisfies `plot` but not `legacy`.
        let result = dependents(&records, concrete("pandas", "2.2.0", "0"));
        let extras: Vec<_> = result
            .iter()
            .filter(|d| d.record.package_record.name.as_normalized() == "extras-tool")
            .map(|d| d.kind.clone())
            .collect();
        assert_eq!(
            extras,
            vec![DependencyKind::ExtraDepends("plot".to_string())]
        );
    }

    #[test]
    fn test_run_exports() {
        let records = test_records();
        // zlib references libzlib both through `depends` and through its
        // weak run exports; both edges are reported.
        let result = dependents(&records, name("libzlib"));
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
        let result = dependents(&records, concrete("libzlib", "2.1", "h0_0"));
        assert!(result.is_empty());

        let result = dependents(&records, concrete("libzlib", "1.3.5", "h0_0"));
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
        let result = dependents(&records, name("__cuda"));
        assert_eq!(names(&result), vec!["cuda-tool"]);

        // A concrete virtual package matches against the dependency's
        // version and build string constraints.
        let cuda = |version: &str| GenericVirtualPackage {
            name: "__cuda".parse().unwrap(),
            version: version.parse().unwrap(),
            build_string: "0".to_string(),
        };
        let result = dependents(&records, cuda("12.4"));
        assert_eq!(names(&result), vec!["cuda-tool"]);
        let result = dependents(&records, cuda("11.8"));
        assert!(result.is_empty());
    }
}
