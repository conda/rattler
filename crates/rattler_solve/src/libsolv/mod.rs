use crate::{SolveError, SolverBackend, SolverProblem};
pub use input::cache_repodata;
use input::{add_repodata_records, add_solv_file, add_virtual_packages};
pub use libc_byte_slice::LibcByteSlice;
use output::get_required_packages;
use rattler_conda_types::RepoDataRecord;
use std::collections::HashMap;
use wrapper::{
    flags::SolverFlag,
    pool::{Pool, Verbosity},
    repo::Repo,
    solve_goal::SolveGoal,
};

mod input;
mod libc_byte_slice;
mod output;
mod wrapper;

/// Represents the information required to load available packages into libsolv for a single channel
/// and platform combination
#[derive(Clone)]
pub struct LibsolvRepoData<'a> {
    /// The actual records after parsing `repodata.json`
    pub records: &'a [RepoDataRecord],

    /// The in-memory .solv file built from the records (if available)
    pub solv_file: Option<&'a LibcByteSlice>,
}

impl LibsolvRepoData<'_> {
    /// Constructs a new `LibsolvRepoData` without a corresponding .solv file
    pub fn from_records(records: &[RepoDataRecord]) -> LibsolvRepoData {
        LibsolvRepoData {
            records,
            solv_file: None,
        }
    }
}

/// A [`SolverBackend`] implemented using the `libsolv` library
pub struct LibsolvBackend;

impl SolverBackend for LibsolvBackend {
    type RepoData<'a> = LibsolvRepoData<'a>;

    fn solve<'a, TAvailablePackagesIterator: Iterator<Item = Self::RepoData<'a>>>(
        &mut self,
        problem: SolverProblem<TAvailablePackagesIterator>,
    ) -> Result<Vec<RepoDataRecord>, SolveError> {
        // Construct a default libsolv pool
        let pool = Pool::default();

        // Setup proper logging for the pool
        pool.set_debug_callback(|msg, flags| {
            tracing::event!(tracing::Level::DEBUG, flags, "{}", msg);
        });
        pool.set_debug_level(Verbosity::Low);

        // Add virtual packages
        let repo = Repo::new(&pool, "virtual_packages");
        add_virtual_packages(&pool, &repo, &problem.virtual_packages)
            .map_err(SolveError::ErrorAddingInstalledPackages)?;

        // Mark the virtual packages as installed.
        pool.set_installed(&repo);

        // Create repos for all channel + platform combinations
        let mut repo_mapping = HashMap::new();
        let mut all_repodata_records = Vec::new();
        for repodata in problem.available_packages {
            if repodata.records.is_empty() {
                continue;
            }

            let channel_name = &repodata.records[0].channel;
            let repo = Repo::new(&pool, channel_name);

            if let Some(solv_file) = repodata.solv_file {
                add_solv_file(&pool, &repo, solv_file);
            } else {
                add_repodata_records(&pool, &repo, repodata.records)
                    .map_err(SolveError::ErrorAddingRepodata)?;
            }

            // Keep our own info about repodata_records
            repo_mapping.insert(repo.id(), repo_mapping.len());
            all_repodata_records.push(repodata.records);

            // We dont want to drop the Repo, its stored in the pool anyway, so just forget it.
            std::mem::forget(repo);
        }

        // Create a special pool for records that are already installed or locked.
        let repo = Repo::new(&pool, "locked");
        let installed_solvables = add_repodata_records(&pool, &repo, &problem.locked_packages)
            .map_err(SolveError::ErrorAddingRepodata)?;

        // Also add the installed records to the repodata
        repo_mapping.insert(repo.id(), repo_mapping.len());
        all_repodata_records.push(&problem.locked_packages);

        // Create a special pool for records that are pinned and cannot be changed.
        let repo = Repo::new(&pool, "pinned");
        let pinned_solvables = add_repodata_records(&pool, &repo, &problem.pinned_packages)
            .map_err(SolveError::ErrorAddingRepodata)?;

        // Also add the installed records to the repodata
        repo_mapping.insert(repo.id(), repo_mapping.len());
        all_repodata_records.push(&problem.pinned_packages);

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
            let id = pool.intern_matchspec(&spec);
            goal.install(id, false)
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = pool.create_solver();
        solver.set_flag(SolverFlag::allow_uninstall(), true);
        solver.set_flag(SolverFlag::allow_downgrade(), true);

        let transaction = solver
            .solve(&mut goal)
            .map_err(|_| SolveError::Unsolvable)?;

        let required_records = get_required_packages(
            &pool,
            &repo_mapping,
            &transaction,
            all_repodata_records.as_slice(),
        )
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
