//! Provides an solver implementation based on the [`rattler_libsolv_rs`] crate.

use crate::{IntoRepoData, SolverRepoData};
use crate::{SolveError, SolverTask};
use input::{add_repodata_records, add_virtual_packages};
use output::get_required_packages;
use rattler_conda_types::MatchSpec;
use rattler_conda_types::RepoDataRecord;
use rattler_libsolv_rs::{CondaDependencyProvider, Pool, SolveJobs, Solver as LibSolvRsSolver};
use std::collections::HashMap;

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
        let mut pool = Pool::<MatchSpec>::new();

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
            add_repodata_records(&mut pool, repo_id, repodata.records.iter().copied())?;

            // Keep our own info about repodata_records
            repo_mapping.insert(repo_id, repo_mapping.len());
            all_repodata_records.push(repodata.records);
        }

        // Create a special pool for records that are already installed or locked.
        let repo_id = pool.new_repo();
        let installed_solvables = add_repodata_records(&mut pool, repo_id, &task.locked_packages)?;

        // Also add the installed records to the repodata
        repo_mapping.insert(repo_id, repo_mapping.len());
        all_repodata_records.push(task.locked_packages.iter().collect());

        // Create a special pool for records that are pinned and cannot be changed.
        let repo_id = pool.new_repo();
        let pinned_solvables = add_repodata_records(&mut pool, repo_id, &task.pinned_packages)?;

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
            let match_spec_id = pool.intern_version_set(dependency_name, spec);
            goal.install(match_spec_id);
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = LibSolvRsSolver::new(pool, CondaDependencyProvider);
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
