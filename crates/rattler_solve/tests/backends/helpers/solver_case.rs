#![cfg(feature = "experimental_conditionals")]

use std::str::FromStr;

use rattler_conda_types::{GenericVirtualPackage, MatchSpec, ParseStrictness, RepoDataRecord};
use rattler_solve::{SolverImpl, SolverTask};

/// Shared building blocks that keep the integration tests concise and data driven.
///
/// Scenarios are expressed as self-documenting cases that describe:
///
/// * the synthetic repositories involved in the test;
/// * the specs that are handed to the solver; and
/// * the packages that must be present or absent in the resulting solution.
///
/// Each failure reports the name of the scenario along with the concrete package set,
/// which makes debugging multi-package situations far easier than chasing a forest of
/// standalone assertions.
#[derive(Clone)]
pub struct SolverCase<'a> {
    name: &'a str,
    repositories: Vec<Vec<RepoDataRecord>>,
    specs: Vec<MatchSpec>,
    virtual_packages: Vec<GenericVirtualPackage>,
    expect_present: Vec<PkgMatcher>,
    expect_absent: Vec<PkgMatcher>,
}

impl<'a> SolverCase<'a> {
    /// Starts a new scenario with a human-readable name surfaced on failure.
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            repositories: Vec::new(),
            specs: Vec::new(),
            virtual_packages: Vec::new(),
            expect_present: Vec::new(),
            expect_absent: Vec::new(),
        }
    }

    /// Adds a synthetic repository snapshot to the scenario.
    pub fn repository(mut self, repo: impl IntoIterator<Item = RepoDataRecord>) -> Self {
        self.repositories.push(repo.into_iter().collect());
        self
    }

    /// Replaces the specs that should be handed to the solver.
    pub fn specs(mut self, specs: impl IntoIterator<Item = &'a str>) -> Self {
        self.specs = specs
            .into_iter()
            .map(|spec| MatchSpec::from_str(spec, ParseStrictness::Lenient).unwrap())
            .collect();
        self
    }

    /// Provides the virtual packages in scope for this scenario.
    pub fn virtual_packages(mut self, packages: Vec<GenericVirtualPackage>) -> Self {
        self.virtual_packages = packages;
        self
    }

    /// Registers packages that must appear in the solver result.
    pub fn expect_present<I, M>(mut self, pkgs: I) -> Self
    where
        I: IntoIterator<Item = M>,
        M: IntoPkgMatcher,
    {
        self.expect_present
            .extend(pkgs.into_iter().map(|pkg| pkg.into_pkg_matcher()));
        self
    }

    /// Registers packages that must not appear in the solver result.
    pub fn expect_absent<I, M>(mut self, pkgs: I) -> Self
    where
        I: IntoIterator<Item = M>,
        M: IntoPkgMatcher,
    {
        self.expect_absent
            .extend(pkgs.into_iter().map(|pkg| pkg.into_pkg_matcher()));
        self
    }

    pub fn run<T: SolverImpl + Default>(&self) {
        let repo_refs: Vec<_> = self.repositories.iter().collect();
        let task = SolverTask {
            specs: self.specs.clone(),
            virtual_packages: self.virtual_packages.clone(),
            ..SolverTask::from_iter(repo_refs)
        };

        let solution = T::default().solve(task).unwrap_or_else(|err| {
            panic!("solver case '{}' failed: {err:?}", self.name);
        });

        assert_expectations(self.name, &solution.records, &self.expect_present, true);
        assert_expectations(self.name, &solution.records, &self.expect_absent, false);
    }
}

/// Executes all supplied scenarios against the target solver implementation.
pub fn run_solver_cases<T: SolverImpl + Default>(cases: &[SolverCase<'_>]) {
    for case in cases {
        case.run::<T>();
    }
}

#[derive(Clone)]
struct PkgMatcher {
    display: String,
    kind: MatcherKind,
}

impl PkgMatcher {
    fn matches(&self, record: &RepoDataRecord) -> bool {
        match &self.kind {
            MatcherKind::Name { name } => record.package_record.name.as_normalized() == name,
            MatcherKind::NameVersion { name, version } => {
                record.package_record.name.as_normalized() == name
                    && record.package_record.version.as_str() == version.as_str()
            }
            MatcherKind::NameVersionBuild {
                name,
                version,
                build,
            } => {
                record.package_record.name.as_normalized() == name
                    && record.package_record.version.as_str() == version.as_str()
                    && record.package_record.build == *build
            }
            MatcherKind::Exact { fingerprint } => fingerprint.matches(record),
        }
    }
}

#[derive(Clone)]
enum MatcherKind {
    Name {
        name: String,
    },
    NameVersion {
        name: String,
        version: String,
    },
    NameVersionBuild {
        name: String,
        version: String,
        build: String,
    },
    Exact {
        fingerprint: PackageFingerprint,
    },
}

#[derive(Clone)]
struct PackageFingerprint {
    name: String,
    version: String,
    build: String,
    build_number: u64,
    channel: Option<String>,
    subdir: String,
    file_name: String,
}

impl PackageFingerprint {
    fn new(record: &RepoDataRecord) -> Self {
        Self {
            name: record.package_record.name.as_normalized().to_string(),
            version: record.package_record.version.as_str().to_string(),
            build: record.package_record.build.clone(),
            build_number: record.package_record.build_number,
            channel: record.channel.clone(),
            subdir: record.package_record.subdir.clone(),
            file_name: record.file_name.clone(),
        }
    }

    fn matches(&self, record: &RepoDataRecord) -> bool {
        self.name == record.package_record.name.as_normalized()
            && self.version == record.package_record.version.as_str()
            && self.build == record.package_record.build
            && self.build_number == record.package_record.build_number
            && self.channel == record.channel
            && self.subdir == record.package_record.subdir
            && self.file_name == record.file_name
    }
}

pub trait IntoPkgMatcher {
    fn into_pkg_matcher(self) -> PkgMatcher;
}

impl<'a> IntoPkgMatcher for &'a RepoDataRecord {
    fn into_pkg_matcher(self) -> PkgMatcher {
        PkgMatcher {
            display: format!(
                "{}={}={}",
                self.package_record.name.as_normalized(),
                self.package_record.version,
                self.package_record.build
            ),
            kind: MatcherKind::Exact {
                fingerprint: PackageFingerprint::new(self),
            },
        }
    }
}

impl IntoPkgMatcher for RepoDataRecord {
    fn into_pkg_matcher(self) -> PkgMatcher {
        (&self).into_pkg_matcher()
    }
}

impl IntoPkgMatcher for &str {
    fn into_pkg_matcher(self) -> PkgMatcher {
        PkgMatcher {
            display: self.to_string(),
            kind: MatcherKind::Name {
                name: self.to_string(),
            },
        }
    }
}

impl IntoPkgMatcher for String {
    fn into_pkg_matcher(self) -> PkgMatcher {
        PkgMatcher {
            display: self.clone(),
            kind: MatcherKind::Name { name: self },
        }
    }
}

impl<'a> IntoPkgMatcher for (&'a str, &'a str) {
    fn into_pkg_matcher(self) -> PkgMatcher {
        PkgMatcher {
            display: format!("{}={}", self.0, self.1),
            kind: MatcherKind::NameVersion {
                name: self.0.to_string(),
                version: self.1.to_string(),
            },
        }
    }
}

impl<'a> IntoPkgMatcher for (&'a str, &'a str, &'a str) {
    fn into_pkg_matcher(self) -> PkgMatcher {
        PkgMatcher {
            display: format!("{}={}={}", self.0, self.1, self.2),
            kind: MatcherKind::NameVersionBuild {
                name: self.0.to_string(),
                version: self.1.to_string(),
                build: self.2.to_string(),
            },
        }
    }
}

fn assert_expectations(
    case: &str,
    records: &[RepoDataRecord],
    matchers: &[PkgMatcher],
    should_exist: bool,
) {
    for matcher in matchers {
        let found = records.iter().any(|record| matcher.matches(record));
        match (should_exist, found) {
            (true, true) | (false, false) => continue,
            (true, false) => panic!(
                "solver case '{case}' expected {} to be present, found packages: {}",
                matcher.display,
                format_records(records)
            ),
            (false, true) => panic!(
                "solver case '{case}' expected {} to be absent, found packages: {}",
                matcher.display,
                format_records(records)
            ),
        }
    }
}

fn format_records(records: &[RepoDataRecord]) -> String {
    if records.is_empty() {
        return "<empty>".to_string();
    }

    records
        .iter()
        .map(|record| {
            format!(
                "{}={}={}",
                record.package_record.name.as_normalized(),
                record.package_record.version,
                record.package_record.build
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}
