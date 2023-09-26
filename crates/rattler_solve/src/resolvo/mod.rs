//! Provides an solver implementation based on the [`resolvo`] crate.

use crate::{IntoRepoData, SolveError, SolverRepoData, SolverTask};
use rattler_conda_types::package::ArchiveType;
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, NamelessMatchSpec, PackageRecord, ParseMatchSpecError,
    RepoDataRecord,
};
use resolvo::{
    Candidates, Dependencies, DependencyProvider, NameId, Pool, SolvableDisplay, SolvableId,
    Solver as LibSolvRsSolver, SolverCache, VersionSet, VersionSetId,
};
use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::HashMap,
    fmt::{Display, Formatter},
    marker::PhantomData,
    ops::Deref,
    str::FromStr,
};

use itertools::Itertools;

mod conda_util;

/// Represents the information required to load available packages into libsolv for a single channel
/// and platform combination
#[derive(Clone)]
pub struct RepoData<'a> {
    /// The actual records after parsing `repodata.json`
    pub records: Vec<&'a RepoDataRecord>,
}

impl<'a> FromIterator<&'a RepoDataRecord> for RepoData<'a> {
    fn from_iter<T: IntoIterator<Item = &'a RepoDataRecord>>(iter: T) -> Self {
        Self {
            records: Vec::from_iter(iter),
        }
    }
}

impl<'a> SolverRepoData<'a> for RepoData<'a> {}

/// Wrapper around `MatchSpec` so that we can use it in the `resolvo` pool
#[repr(transparent)]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct SolverMatchSpec<'a> {
    inner: NamelessMatchSpec,
    _marker: PhantomData<&'a PackageRecord>,
}

impl<'a> From<NamelessMatchSpec> for SolverMatchSpec<'a> {
    fn from(value: NamelessMatchSpec) -> Self {
        Self {
            inner: value,
            _marker: Default::default(),
        }
    }
}

impl<'a> Display for SolverMatchSpec<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<'a> Deref for SolverMatchSpec<'a> {
    type Target = NamelessMatchSpec;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> VersionSet for SolverMatchSpec<'a> {
    type V = SolverPackageRecord<'a>;

    fn contains(&self, v: &Self::V) -> bool {
        match v {
            SolverPackageRecord::Record(rec) => self.inner.matches(&rec.package_record),
            SolverPackageRecord::VirtualPackage(GenericVirtualPackage {
                version,
                build_string,
                ..
            }) => {
                if let Some(spec) = self.inner.version.as_ref() {
                    if !spec.matches(version) {
                        return false;
                    }
                }

                if let Some(build_match) = self.inner.build.as_ref() {
                    if !build_match.matches(build_string) {
                        return false;
                    }
                }

                true
            }
        }
    }
}

/// Wrapper around [`PackageRecord`] so that we can use it in resolvo pool
#[derive(Ord, PartialOrd, Eq, PartialEq)]
enum SolverPackageRecord<'a> {
    Record(&'a RepoDataRecord),
    VirtualPackage(&'a GenericVirtualPackage),
}

impl<'a> SolverPackageRecord<'a> {
    fn version(&self) -> &rattler_conda_types::Version {
        match self {
            SolverPackageRecord::Record(rec) => rec.package_record.version.version(),
            SolverPackageRecord::VirtualPackage(rec) => &rec.version,
        }
    }

    fn track_features(&self) -> &[String] {
        const EMPTY: [String; 0] = [];
        match self {
            SolverPackageRecord::Record(rec) => &rec.package_record.track_features,
            SolverPackageRecord::VirtualPackage(_rec) => &EMPTY,
        }
    }

    fn build_number(&self) -> u64 {
        match self {
            SolverPackageRecord::Record(rec) => rec.package_record.build_number,
            SolverPackageRecord::VirtualPackage(_rec) => 0,
        }
    }

    fn timestamp(&self) -> Option<&chrono::DateTime<chrono::Utc>> {
        match self {
            SolverPackageRecord::Record(rec) => rec.package_record.timestamp.as_ref(),
            SolverPackageRecord::VirtualPackage(_rec) => None,
        }
    }
}

impl<'a> Display for SolverPackageRecord<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SolverPackageRecord::Record(rec) => {
                write!(f, "{}", &rec.package_record)
            }
            SolverPackageRecord::VirtualPackage(rec) => {
                write!(f, "{}", rec)
            }
        }
    }
}

/// Dependency provider for conda
#[derive(Default)]
pub(crate) struct CondaDependencyProvider<'a> {
    pool: Pool<SolverMatchSpec<'a>, String>,

    records: HashMap<NameId, Candidates>,

    matchspec_to_highest_version:
        RefCell<HashMap<VersionSetId, Option<(rattler_conda_types::Version, bool)>>>,

    parse_match_spec_cache: RefCell<HashMap<&'a str, VersionSetId>>,
}

impl<'a> CondaDependencyProvider<'a> {
    pub fn from_solver_task(
        repodata: impl IntoIterator<Item = RepoData<'a>>,
        favored_records: &'a [RepoDataRecord],
        locked_records: &'a [RepoDataRecord],
        virtual_packages: &'a [GenericVirtualPackage],
    ) -> Self {
        let pool = Pool::default();
        let mut records: HashMap<NameId, Candidates> = HashMap::default();

        // Add virtual packages to the records
        for virtual_package in virtual_packages {
            let name = pool.intern_package_name(virtual_package.name.as_normalized());
            let solvable =
                pool.intern_solvable(name, SolverPackageRecord::VirtualPackage(virtual_package));
            records.entry(name).or_default().candidates.push(solvable);
        }

        // Add additional records
        for repo_datas in repodata {
            // Iterate over all records and dedup records that refer to the same package data but with
            // different archive types. This can happen if you have two variants of the same package but
            // with different extensions. We prefer `.conda` packages over `.tar.bz`.
            //
            // Its important to insert the records in the same same order as how they were presented to this
            // function to ensure that each solve is deterministic. Iterating over HashMaps is not
            // deterministic at runtime so instead we store the values in a Vec as we iterate over the
            // records. This guarentees that the order of records remains the same over runs.
            let mut ordered_repodata = Vec::with_capacity(repo_datas.records.len());
            let mut package_to_type: HashMap<&str, (ArchiveType, usize)> =
                HashMap::with_capacity(repo_datas.records.len());

            for record in repo_datas.records {
                let (file_name, archive_type) = ArchiveType::split_str(&record.file_name)
                    .unwrap_or((&record.file_name, ArchiveType::TarBz2));
                match package_to_type.get_mut(file_name) {
                    None => {
                        let idx = ordered_repodata.len();
                        ordered_repodata.push(record);
                        package_to_type.insert(file_name, (archive_type, idx));
                    }
                    Some((prev_archive_type, idx)) => match archive_type.cmp(prev_archive_type) {
                        Ordering::Greater => {
                            // A previous package has a worse package "type", we'll use the current record
                            // instead.
                            *prev_archive_type = archive_type;
                            ordered_repodata[*idx] = record;
                        }
                        Ordering::Less => {
                            // A previous package that we already stored is actually a package of a better
                            // "type" so we'll just use that instead (.conda > .tar.bz)
                        }
                        Ordering::Equal => {
                            if record != ordered_repodata[*idx] {
                                unreachable!(
                                    "found duplicate record with different values for {}",
                                    &record.file_name
                                );
                            }
                        }
                    },
                }
            }

            for record in ordered_repodata {
                let package_name =
                    pool.intern_package_name(record.package_record.name.as_normalized());
                let solvable_id =
                    pool.intern_solvable(package_name, SolverPackageRecord::Record(record));
                let candidates = records.entry(package_name).or_default();
                candidates.candidates.push(solvable_id);
                candidates.hint_dependencies_available.push(solvable_id);
            }
        }

        // Add favored packages to the records
        for favored_record in favored_records {
            let name = pool.intern_package_name(favored_record.package_record.name.as_normalized());
            let solvable = pool.intern_solvable(name, SolverPackageRecord::Record(favored_record));
            let mut candidates = records.entry(name).or_default();
            candidates.candidates.push(solvable);
            candidates.favored = Some(solvable);
        }

        for locked_record in locked_records {
            let name = pool.intern_package_name(locked_record.package_record.name.as_normalized());
            let solvable = pool.intern_solvable(name, SolverPackageRecord::Record(locked_record));
            let mut candidates = records.entry(name).or_default();
            candidates.candidates.push(solvable);
            candidates.locked = Some(solvable);
        }

        Self {
            pool,
            records,
            matchspec_to_highest_version: Default::default(),
            parse_match_spec_cache: Default::default(),
        }
    }
}

impl<'a> DependencyProvider<SolverMatchSpec<'a>> for CondaDependencyProvider<'a> {
    fn pool(&self) -> &Pool<SolverMatchSpec<'a>, String> {
        &self.pool
    }

    fn sort_candidates(
        &self,
        solver: &SolverCache<SolverMatchSpec<'a>, String, Self>,
        solvables: &mut [SolvableId],
    ) {
        let mut highest_version_spec = self.matchspec_to_highest_version.borrow_mut();
        solvables.sort_by(|&p1, &p2| {
            conda_util::compare_candidates(p1, p2, solver, &mut highest_version_spec)
        });
    }

    fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        self.records.get(&name).cloned()
    }

    fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        let SolverPackageRecord::Record(rec) = self.pool.resolve_solvable(solvable).inner() else { return Dependencies::default() };

        let mut parse_match_spec_cache = self.parse_match_spec_cache.borrow_mut();
        let mut dependencies = Dependencies::default();
        for depends in rec.package_record.depends.iter() {
            let version_set_id =
                parse_match_spec(&self.pool, depends, &mut parse_match_spec_cache).unwrap();
            dependencies.requirements.push(version_set_id);
        }

        for constrains in rec.package_record.constrains.iter() {
            let version_set_id =
                parse_match_spec(&self.pool, constrains, &mut parse_match_spec_cache).unwrap();
            dependencies.constrains.push(version_set_id);
        }

        dependencies
    }
}

/// Displays the different candidates by their version and sorted by their version
pub struct CondaSolvableDisplay;

impl SolvableDisplay<SolverMatchSpec<'_>> for CondaSolvableDisplay {
    fn display_candidates(
        &self,
        pool: &Pool<SolverMatchSpec, String>,
        merged_candidates: &[SolvableId],
    ) -> String {
        merged_candidates
            .iter()
            .map(|&id| pool.resolve_solvable(id).inner().version())
            .sorted()
            .map(|s| s.to_string())
            .join(" | ")
    }
}

/// A [`Solver`] implemented using the `resolvo` library
#[derive(Default)]
pub struct Solver;

impl super::SolverImpl for Solver {
    type RepoData<'a> = RepoData<'a>;

    fn solve<
        'a,
        R: IntoRepoData<'a, Self::RepoData<'a>>,
        TAvailablePackagesIterator: IntoIterator<Item = R>,
    >(
        &mut self,
        task: SolverTask<TAvailablePackagesIterator>,
    ) -> Result<Vec<RepoDataRecord>, SolveError> {
        // Construct a provider that can serve the data.
        let provider = CondaDependencyProvider::from_solver_task(
            task.available_packages.into_iter().map(|r| r.into()),
            &task.locked_packages,
            &task.pinned_packages,
            &task.virtual_packages,
        );

        // Construct the requirements that the solver needs to satisfy.
        let root_requirements = task
            .specs
            .into_iter()
            .map(|spec| {
                let (name, spec) = spec.into_nameless();
                let name = name.expect("cannot use matchspec without a name");
                let name_id = provider.pool.intern_package_name(name.as_normalized());
                provider.pool.intern_version_set(name_id, spec.into())
            })
            .collect();

        // Construct a solver and solve the problems in the queue
        let mut solver = LibSolvRsSolver::new(provider);
        let solvables = solver.solve(root_requirements).map_err(|problem| {
            SolveError::Unsolvable(vec![problem
                .display_user_friendly(&solver, &CondaSolvableDisplay)
                .to_string()])
        })?;

        // Get the resulting packages from the solver.
        let required_records = solvables
            .into_iter()
            .filter_map(|id| match solver.pool().resolve_solvable(id).inner() {
                SolverPackageRecord::Record(rec) => Some(rec.deref().clone()),
                SolverPackageRecord::VirtualPackage(_) => None,
            })
            .collect();

        Ok(required_records)
    }
}

fn parse_match_spec<'a>(
    pool: &Pool<SolverMatchSpec<'a>>,
    spec_str: &'a str,
    parse_match_spec_cache: &mut HashMap<&'a str, VersionSetId>,
) -> Result<VersionSetId, ParseMatchSpecError> {
    Ok(match parse_match_spec_cache.get(spec_str) {
        Some(spec_id) => *spec_id,
        None => {
            let match_spec = MatchSpec::from_str(spec_str)?;
            let (name, spec) = match_spec.into_nameless();
            let dependency_name = pool.intern_package_name(
                name.as_ref()
                    .expect("match specs without names are not supported")
                    .as_normalized(),
            );
            let version_set_id = pool.intern_version_set(dependency_name, spec.into());
            parse_match_spec_cache.insert(spec_str, version_set_id);
            version_set_id
        }
    })
}
