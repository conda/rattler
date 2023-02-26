use rattler_conda_types::RepoDataRecord;
use std::collections::HashMap;

mod wrapper;

use crate::{SolveError, SolverBackend, SolverProblem};
use wrapper::{
    flags::SolverFlag,
    pool::Intern,
    pool::{Pool, Verbosity},
    solve_goal::SolveGoal,
};

/// A [`SolverBackend`] implemented using the `libsolv` library
pub struct LibsolvSolver;

impl SolverBackend for LibsolvSolver {
    fn solve(&mut self, problem: SolverProblem) -> Result<Vec<RepoDataRecord>, SolveError> {
        // Construct a default libsolv pool
        let pool = Pool::default();

        // Setup proper logging for the pool
        pool.set_debug_callback(|msg, flags| {
            tracing::event!(tracing::Level::DEBUG, flags, "{}", msg);
        });
        pool.set_debug_level(Verbosity::Extreme);

        // Add virtual packages
        let repo = pool.create_repo("virtual_packages");
        repo.add_virtual_packages(&pool, &problem.virtual_packages)
            .map_err(SolveError::ErrorAddingInstalledPackages)?;

        // Mark the virtual packages as installed.
        pool.set_installed(&repo);

        // Create repos for all channels
        let mut repo_mapping = HashMap::with_capacity(problem.available_packages.len() + 1);
        let mut all_repodata_records = Vec::with_capacity(repo_mapping.len());
        for repodata_records in &problem.available_packages {
            if repodata_records.is_empty() {
                continue;
            }

            let channel_name = &repodata_records[0].channel;
            let repo = pool.create_repo(channel_name);
            repo.add_repodata_records(&pool, repodata_records)
                .map_err(SolveError::ErrorAddingRepodata)?;

            // Keep our own info about repodata_records
            repo_mapping.insert(repo.id(), repo_mapping.len());
            all_repodata_records.push(repodata_records.as_slice());

            // We dont want to drop the Repo, its stored in the pool anyway, so just forget it.
            std::mem::forget(repo);
        }

        // Create a special pool for records that are already installed or locked.
        let repo = pool.create_repo("installed");
        let installed_solvables = repo
            .add_repodata_records(&pool, &problem.locked_packages)
            .map_err(SolveError::ErrorAddingInstalledPackages)?;

        // Also add the installed records to the repodata
        repo_mapping.insert(repo.id(), repo_mapping.len());
        all_repodata_records.push(problem.locked_packages.as_slice());

        // Create a special pool for records that are pinned and cannot be changed.
        let repo = pool.create_repo("pinned");
        let pinned_solvables = repo
            .add_repodata_records(&pool, &problem.pinned_packages)
            .map_err(SolveError::ErrorAddingInstalledPackages)?;

        // Also add the installed records to the repodata
        repo_mapping.insert(repo.id(), repo_mapping.len());
        all_repodata_records.push(problem.pinned_packages.as_slice());

        // Create datastructures for solving
        pool.create_whatprovides();

        // Add matchspec to the queue
        let mut goal = SolveGoal::default();

        // Favor the currently installed packages
        for favor_solvable in installed_solvables {
            goal.favor(favor_solvable);
        }

        // Lock the currently pinned packages
        for locked_solvable in pinned_solvables {
            goal.lock(locked_solvable);
        }

        // Specify the matchspec requests
        for spec in problem.specs {
            let id = spec.intern(&pool);
            goal.install(id, false)
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = pool.create_solver();
        solver.set_flag(SolverFlag::allow_uninstall(), true);
        solver.set_flag(SolverFlag::allow_downgrade(), true);
        if solver.solve(&mut goal).is_err() {
            return Err(SolveError::Unsolvable);
        }

        // Construct a transaction from the solver
        let mut transaction = solver.create_transaction();
        let required_records = transaction
            .get_required_packages(&pool, &repo_mapping, &all_repodata_records)
            .map_err(|unsupported_operation_ids| {
                SolveError::UnsupportedOperations(
                    unsupported_operation_ids
                        .into_iter()
                        .map(|id| format!("libsolv operation {id}"))
                        .collect(),
                )
            })?;

        Ok(required_records)
    }
}
