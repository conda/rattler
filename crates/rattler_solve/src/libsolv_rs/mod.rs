//! Provides an solver implementation based on the [`rattler_libsolv_rs`] crate.

use crate::{IntoRepoData, SolveError, SolverRepoData, SolverTask};
use input::{add_repodata_records, add_virtual_packages};
use output::get_required_packages;
use rattler_conda_types::{MatchSpec, PackageRecord, RepoDataRecord};
use rattler_libsolv_rs::{
    DependencyProvider, Mapping, Pool, SolvableId, SolveJobs, Solver as LibSolvRsSolver,
    VersionSet, VersionSetId, VersionTrait,
};
use ref_cast::RefCast;
use std::{
    cell::OnceCell,
    collections::HashMap,
    fmt::{Display, Formatter},
    marker::PhantomData,
    ops::Deref,
};

mod conda_util;
mod input;
mod output;

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
pub struct SolverMatchSpec<'a> {
    inner: MatchSpec,
    _marker: PhantomData<&'a PackageRecord>,
}

impl<'a> From<MatchSpec> for SolverMatchSpec<'a> {
    fn from(value: MatchSpec) -> Self {
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
    type Target = MatchSpec;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> VersionSet for SolverMatchSpec<'a> {
    type V = &'a SolverPackageRecord;

    fn contains(&self, v: &Self::V) -> bool {
        self.matches(v.deref())
    }
}

/// Wrapper around [`PackageRecord`] so that we can use it in libsolv_rs pool
#[derive(RefCast)]
#[repr(transparent)]
pub struct SolverPackageRecord(PackageRecord);

impl Deref for SolverPackageRecord {
    type Target = PackageRecord;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for SolverPackageRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'a> VersionTrait for &'a SolverPackageRecord {
    type Name = String;
    type Version = rattler_conda_types::Version;

    fn version(&self) -> Self::Version {
        self.0.version.version().clone()
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
        let repo_id = pool.new_repo();
        add_virtual_packages(&mut pool, repo_id, &task.virtual_packages);

        // Create repos for all channel + platform combinations
        let mut repo_mapping = HashMap::new();
        let mut all_repodata_records = Vec::new();
        for repodata in task.available_packages.into_iter().map(IntoRepoData::into) {
            if repodata.records.is_empty() {
                continue;
            }

            let repo_id = pool.new_repo();
            add_repodata_records(
                &mut pool,
                repo_id,
                repodata.records.iter().copied(),
                &mut parse_match_spec_cache,
            )?;

            // Keep our own info about repodata_records
            repo_mapping.insert(repo_id, repo_mapping.len());
            all_repodata_records.push(repodata.records);
        }

        // Create a special pool for records that are already installed or locked.
        let repo_id = pool.new_repo();
        let installed_solvables = add_repodata_records(
            &mut pool,
            repo_id,
            &task.locked_packages,
            &mut parse_match_spec_cache,
        )?;

        // Also add the installed records to the repodata
        repo_mapping.insert(repo_id, repo_mapping.len());
        all_repodata_records.push(task.locked_packages.iter().collect());

        // Create a special pool for records that are pinned and cannot be changed.
        let repo_id = pool.new_repo();
        let pinned_solvables = add_repodata_records(
            &mut pool,
            repo_id,
            &task.pinned_packages,
            &mut parse_match_spec_cache,
        )?;

        // Also add the installed records to the repodata
        repo_mapping.insert(repo_id, repo_mapping.len());
        all_repodata_records.push(task.pinned_packages.iter().collect());

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
            let match_spec_id = pool.intern_version_set(dependency_name, spec.into());
            goal.install(match_spec_id);
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = LibSolvRsSolver::new(pool, CondaDependencyProvider::default());
        let transaction = solver.solve(goal).map_err(|problem| {
            SolveError::Unsolvable(vec![problem.display_user_friendly(&solver).to_string()])
        })?;

        let required_records = get_required_packages(
            solver.pool(),
            &repo_mapping,
            &transaction,
            all_repodata_records.as_slice(),
        );

        Ok(required_records)
    }
}
