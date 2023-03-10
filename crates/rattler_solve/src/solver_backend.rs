use crate::{SolveError, SolverTask};
use rattler_conda_types::RepoDataRecord;

/// Represents a solver backend, capable of solving [`SolverTask`]s
pub trait SolverBackend {
    /// The repo data associated to a channel and platform combination
    type RepoData<'a>;

    /// Resolve the dependencies and return the [`RepoDataRecord`]s that should be present in the
    /// environment.
    fn solve<'a, TAvailablePackagesIterator: Iterator<Item = Self::RepoData<'a>>>(
        &mut self,
        task: SolverTask<TAvailablePackagesIterator>,
    ) -> Result<Vec<RepoDataRecord>, SolveError>;
}
