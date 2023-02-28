use crate::{SolveError, SolverProblem};
use rattler_conda_types::RepoDataRecord;

/// Represents a solver backend, capable of solving [`SolverProblem`]s
pub trait SolverBackend {
    /// Resolve the dependencies and return the [`RepoDataRecord`]s that should be present in the
    /// environment.
    fn solve(&mut self, problem: SolverProblem) -> Result<Vec<RepoDataRecord>, SolveError>;
}
