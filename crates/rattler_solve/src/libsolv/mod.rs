use crate::{SolveError, SolverBackend, SolverTask};
use input::{add_repodata_records, add_virtual_packages};
use libsolv_rs::pool::Pool;
use libsolv_rs::solve_jobs::SolveJobs;
use libsolv_rs::solver::Solver;
use output::get_required_packages;
use rattler_conda_types::RepoDataRecord;
use std::collections::HashMap;

mod input;
mod output;

/// Represents the information required to load available packages into libsolv for a single channel
/// and platform combination
#[derive(Clone)]
pub struct LibsolvRepoData<'a> {
    /// The actual records after parsing `repodata.json`
    pub records: &'a [RepoDataRecord],
}

impl LibsolvRepoData<'_> {
    /// Constructs a new `LibsolvRepoData` without a corresponding .solv file
    pub fn from_records(records: &[RepoDataRecord]) -> LibsolvRepoData {
        LibsolvRepoData { records }
    }
}

/// A [`SolverBackend`] implemented using the `libsolv` library
pub struct LibsolvBackend;

impl SolverBackend for LibsolvBackend {
    type RepoData<'a> = LibsolvRepoData<'a>;

    fn solve<'a, TAvailablePackagesIterator: Iterator<Item = Self::RepoData<'a>>>(
        &mut self,
        task: SolverTask<TAvailablePackagesIterator>,
    ) -> Result<Vec<RepoDataRecord>, SolveError> {
        // Construct a default libsolv pool
        let mut pool = Pool::new();

        // Add virtual packages
        let repo_id = pool.new_repo("virtual_packages");
        add_virtual_packages(&mut pool, repo_id, &task.virtual_packages);

        // Create repos for all channel + platform combinations
        let mut repo_mapping = HashMap::new();
        let mut all_repodata_records = Vec::new();
        for repodata in task.available_packages {
            if repodata.records.is_empty() {
                continue;
            }

            let channel_name = &repodata.records[0].channel;
            let repo_id = pool.new_repo(channel_name);
            add_repodata_records(&mut pool, repo_id, repodata.records);

            // Keep our own info about repodata_records
            repo_mapping.insert(repo_id, repo_mapping.len());
            all_repodata_records.push(repodata.records);
        }

        // Create a special pool for records that are already installed or locked.
        let repo_id = pool.new_repo("locked");
        let installed_solvables = add_repodata_records(&mut pool, repo_id, &task.locked_packages);

        // Also add the installed records to the repodata
        repo_mapping.insert(repo_id, repo_mapping.len());
        all_repodata_records.push(&task.locked_packages);

        // Create a special pool for records that are pinned and cannot be changed.
        let repo_id = pool.new_repo("pinned");
        let pinned_solvables = add_repodata_records(&mut pool, repo_id, &task.pinned_packages);

        // Also add the installed records to the repodata
        repo_mapping.insert(repo_id, repo_mapping.len());
        all_repodata_records.push(&task.pinned_packages);

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
            goal.install(spec);
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = Solver::new(pool);
        let transaction = solver.solve(goal).map_err(|problem| {
            SolveError::Unsolvable(vec![problem.display_user_friendly(&solver).to_string()])
        })?;

        let required_records = get_required_packages(
            solver.pool(),
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
