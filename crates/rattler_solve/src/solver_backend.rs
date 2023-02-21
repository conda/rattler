use crate::{PackageOperation, SolveError, SolverProblem};

/// Represents a solver backend, capable of solving [`SolverProblem`]s
pub trait SolverBackend {
    /// Resolve the dependencies and return the required [`PackageOperation`]s in the order in which
    /// they need to be applied
    fn solve(problem: SolverProblem) -> Result<Vec<PackageOperation>, SolveError>;
}
