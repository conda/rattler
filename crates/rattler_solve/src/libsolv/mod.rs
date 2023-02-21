use std::collections::HashMap;

mod wrapper;

use crate::{PackageOperation, SolveError, SolverBackend, SolverProblem};
use wrapper::flags::{SolvableFlags, SolverFlag};
use wrapper::pool::{Intern, Pool, Verbosity};
use wrapper::queue::Queue;

/// A [`SolverBackend`] implemented using the `libsolv` library
pub struct LibsolvSolver;

impl SolverBackend for LibsolvSolver {
    fn solve(problem: SolverProblem) -> Result<Vec<PackageOperation>, SolveError> {
        // Construct a default libsolv pool
        let pool = Pool::default();

        // Setup proper logging for the pool
        pool.set_debug_callback(|msg, flags| {
            tracing::event!(tracing::Level::DEBUG, flags, "{}", msg);
        });
        pool.set_debug_level(Verbosity::Low);

        // Create repos for all channels
        let mut repo_mapping = HashMap::with_capacity(problem.available_packages.len() + 1);
        let mut all_repodata_records = Vec::with_capacity(repo_mapping.len());
        for repodata_records in &problem.available_packages {
            if repodata_records.is_empty() {
                continue;
            }

            let channel_name = &repodata_records[0].channel;
            let repo = pool.create_repo(channel_name);
            repo.add_repodata_records(repodata_records)
                .map_err(SolveError::ErrorAddingRepodata)?;

            // Keep our own info about repodata_records
            let i = repo_mapping.len();
            repo_mapping.insert(repo.id(), i);
            all_repodata_records.push(repodata_records.as_slice());

            // We dont want to drop the Repo, its stored in the pool anyway, so just forget it.
            std::mem::forget(repo);
        }

        // Installed and virtual packages
        let repo = pool.create_repo("installed");
        let installed_records: Vec<_> = problem
            .installed_packages
            .into_iter()
            .map(|p| p.repodata_record)
            .collect();
        repo.add_repodata_records(&installed_records)
            .map_err(SolveError::ErrorAddingInstalledPackages)?;
        repo.add_virtual_packages(&problem.virtual_packages)
            .map_err(SolveError::ErrorAddingInstalledPackages)?;
        pool.set_installed(&repo);

        let i = repo_mapping.len();
        repo_mapping.insert(repo.id(), i);
        all_repodata_records.push(installed_records.as_slice());

        // Create datastructures for solving
        pool.create_whatprovides();

        // Add matchspec to the queue
        let mut queue = Queue::default();
        for (spec, request) in problem.specs {
            let id = spec.intern(&pool);
            queue.push_id_with_flags(id, SolvableFlags::from(request));
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = pool.create_solver();
        solver.set_flag(SolverFlag::allow_uninstall(), true);
        solver.set_flag(SolverFlag::allow_downgrade(), true);
        if solver.solve(&mut queue).is_err() {
            return Err(SolveError::Unsolvable);
        }

        // Construct a transaction from the solver
        let mut transaction = solver.create_transaction();
        let operations = transaction
            .get_package_operations(&repo_mapping, &all_repodata_records)
            .map_err(|unsupported_operation_ids| {
                SolveError::UnsupportedOperations(
                    unsupported_operation_ids
                        .into_iter()
                        .map(|id| format!("libsolv operation {id}"))
                        .collect(),
                )
            })?;

        Ok(operations)
    }
}
