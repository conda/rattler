use crate::{SolveError, SolverProblem};
use rattler_conda_types::RepoDataRecord;

/// Represents a solver backend, capable of solving [`SolverProblem`]s
pub trait SolverBackend {
    /// The repo data associated to a channel and platform combination
    type RepoData<'a>;

    /// Resolve the dependencies and return the [`RepoDataRecord`]s that should be present in the
    /// environment.
    fn solve<'a, TAvailablePackagesIterator: Iterator<Item = Self::RepoData<'a>>>(
        &mut self,
        problem: SolverProblem<TAvailablePackagesIterator>,
    ) -> Result<Vec<RepoDataRecord>, SolveError>;
}
