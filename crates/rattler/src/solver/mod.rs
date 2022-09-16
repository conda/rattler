mod libsolv;

use std::collections::HashMap;

use libsolv::{Intern, Pool, Queue, Verbosity, SOLVER_INSTALL, SOLVER_SOLVABLE_PROVIDES};

use crate::{MatchSpec, RepoData};

use self::libsolv::InstallOperation;

#[derive(thiserror::Error, Debug)]
pub enum SolveError {
    #[error("unsolvable")]
    Unsolvable,

    #[error("error adding repodata: {0}")]
    ErrorAddingRepodata(#[source] anyhow::Error),
}

#[derive(Debug)]
pub struct Entry {
    pub channel: String,
    pub location: String,
    pub operation: InstallOperation,
}

#[derive(Debug, Default)]
pub struct SolverProblem<'c> {
    /// All the available channels (and contents) in order of priority
    pub channels: Vec<(String, &'c RepoData)>,

    /// The specs we want to solve
    pub specs: Vec<MatchSpec>,
}

impl<'c> SolverProblem<'c> {
    pub fn solve(self) -> Result<Vec<Entry>, SolveError> {
        // Construct a default libsolv pool
        let mut pool = Pool::default();

        // Setup proper logging for the pool
        pool.set_debug_callback(|msg, flags| {
            tracing::event!(tracing::Level::DEBUG, flags, "{}", msg);
        });
        pool.set_debug_level(Verbosity::Low);

        // Create repos for all channels
        let mut channel_mapping = HashMap::new();
        for (channel, repodata) in self.channels.iter() {
            let mut repo = pool.create_repo(&channel);
            repo.add_repodata(*repodata)
                .map_err(SolveError::ErrorAddingRepodata)?;
            channel_mapping.insert(repo.id(), channel);

            // We dont want to drop the Repo, its stored in the pool anyway, so just forget it.
            std::mem::forget(repo);
        }

        // Create datastructures for solving
        pool.create_whatprovides();

        // Add matchspec to the queue
        let mut queue = Queue::default();
        for spec in self.specs {
            let id = spec.intern(&mut pool);
            queue.push_id_with_flags(id, (SOLVER_INSTALL | SOLVER_SOLVABLE_PROVIDES) as i32);
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = pool.create_solver();
        if solver.solve(&mut queue).is_err() {
            return Err(SolveError::Unsolvable);
        }

        // Construct a transaction from the solver
        let mut transaction = solver.create_transaction();
        let solvable_operations = transaction.get_solvable_operations();
        let mut operations = Vec::new();
        for operation in solvable_operations.iter() {
            let channel = *channel_mapping
                .get(&operation.solvable.repo().id())
                .expect("could not find previously stored channel");
            let location = operation.solvable.location();
            operations.push(Entry {
                channel: channel.clone(),
                location,
                operation: operation.operation,
            });
        }

        Ok(operations)
    }
}
