//! Provides an solver implementation based on the [`rattler_libsolv_rs`] crate.

use crate::{IntoRepoData, SolveError, SolverRepoData, SolverTask};
use input::{add_repodata_records, add_virtual_packages};
use rattler_conda_types::{
    GenericVirtualPackage, NamelessMatchSpec, PackageRecord, RepoDataRecord,
};
use rattler_libsolv_rs::{
    DependencyProvider, Mapping, Pool, SolvableDisplay, SolvableId, SolveJobs,
    Solver as LibSolvRsSolver, VersionSet, VersionSetId,
};
use std::{
    cell::OnceCell,
    collections::HashMap,
    fmt::{Display, Formatter},
    marker::PhantomData,
    ops::Deref,
};

use itertools::Itertools;

mod conda_util;
mod input;

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

/// Wrapper around `MatchSpec` so that we can use it in the `libsolv_rs` pool
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

/// Wrapper around [`PackageRecord`] so that we can use it in libsolv_rs pool
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
pub(crate) struct CondaDependencyProvider {
    // TODO: cache is dangerous as it is now because it is not invalidated when the pool changes
    // this never happens when the pool is moved, because it belongs to the solver.
    // but if there is a case the solver calls `overwrite_package` on the pool this cache might
    // become invalid.
    // Note that using https://docs.rs/slotmap/latest/slotmap/ instead of our own Arena types
    // could solve this issue as we could store a version with the VersionSetId and check if it
    // is invalidated
    matchspec_to_highest_version:
        HashMap<VersionSetId, Option<(rattler_conda_types::Version, bool)>>,
}

impl<'a> DependencyProvider<SolverMatchSpec<'a>> for CondaDependencyProvider {
    fn sort_candidates(
        &mut self,
        pool: &Pool<SolverMatchSpec<'a>>,
        solvables: &mut [SolvableId],
        match_spec_to_candidates: &Mapping<VersionSetId, OnceCell<Vec<SolvableId>>>,
    ) {
        solvables.sort_by(|&p1, &p2| {
            conda_util::compare_candidates(
                p1,
                p2,
                pool,
                match_spec_to_candidates,
                &mut self.matchspec_to_highest_version,
            )
        });
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

/// A [`Solver`] implemented using the `libsolv` library
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
        // Construct a default libsolv pool
        let mut pool: Pool<SolverMatchSpec> = Pool::new();
        let mut parse_match_spec_cache = HashMap::new();

        // Add virtual packages
        add_virtual_packages(&mut pool, &task.virtual_packages);

        // Create repos for all channel + platform combinations
        for repodata in task.available_packages.into_iter().map(IntoRepoData::into) {
            if repodata.records.is_empty() {
                continue;
            }

            add_repodata_records(
                &mut pool,
                repodata.records.iter().copied(),
                &mut parse_match_spec_cache,
            )?;
        }

        // Create a special pool for records that are already installed or locked.
        let installed_solvables = add_repodata_records(
            &mut pool,
            &task.locked_packages,
            &mut parse_match_spec_cache,
        )?;

        // Create a special pool for records that are pinned and cannot be changed.
        let pinned_solvables = add_repodata_records(
            &mut pool,
            &task.pinned_packages,
            &mut parse_match_spec_cache,
        )?;

        // Add matchspec to the queue
        let mut goal = SolveJobs::default();

        // Favor the currently installed packages
        for favor_solvable in installed_solvables {
            goal.favor(favor_solvable);
        }

        // Lock the currently pinned packages
        for locked_solvable in pinned_solvables {
            goal.lock(locked_solvable);
        }

        // Specify the matchspec requests
        for spec in task.specs {
            let dependency_name = pool.intern_package_name(
                spec.name
                    .as_ref()
                    .expect("match specs without names are not supported")
                    .as_normalized(),
            );
            let match_spec_id =
                pool.intern_version_set(dependency_name, NamelessMatchSpec::from(spec).into());
            goal.install(match_spec_id);
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = LibSolvRsSolver::new(pool, CondaDependencyProvider::default());
        let transaction = solver.solve(goal).map_err(|problem| {
            SolveError::Unsolvable(vec![problem
                .display_user_friendly(&solver, &CondaSolvableDisplay)
                .to_string()])
        })?;

        let required_records = transaction
            .steps
            .into_iter()
            .filter_map(|id| match solver.pool().resolve_solvable(id).inner() {
                SolverPackageRecord::Record(rec) => Some(rec.deref().clone()),
                SolverPackageRecord::VirtualPackage(_) => None,
            })
            .collect();

        Ok(required_records)
    }
}
