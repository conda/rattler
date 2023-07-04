use crate::{SolveError, SolverTask};
use rattler_conda_types::RepoDataRecord;

/// A representation of a collection of [`RepoDataRecord`] usable by a [`SolverBackend`]
/// implementation.
///
/// Some solvers might be able to cache the collection between different runs of the solver which
/// could potentially eliminate some overhead. This trait enables creating a representation of the
/// repodata that is most suitable for a specific backend.
///
/// Some solvers may add additional functionality to their specific implementation that enables
/// caching the repodata to disk in an efficient way (see [`crate::libsolv::LibSolvRepoData`] for
/// an example).
pub trait SolverRepoData<'a>: FromIterator<&'a RepoDataRecord> {}

/// Defines the ability to convert a type into [`SolverRepoData`].
pub trait IntoRepoData<'a, S: SolverRepoData<'a>> {
    fn into(self) -> S;
}

impl<'a, S: SolverRepoData<'a>> IntoRepoData<'a, S> for &'a Vec<RepoDataRecord> {
    fn into(self) -> S {
        S::from_iter(self.iter())
    }
}

impl<'a, S: SolverRepoData<'a>> IntoRepoData<'a, S> for &'a [RepoDataRecord] {
    fn into(self) -> S {
        S::from_iter(self.iter())
    }
}

impl<'a, S: SolverRepoData<'a>> IntoRepoData<'a, S> for S {
    fn into(self) -> S {
        self
    }
}

/// Represents a solver backend, capable of solving [`SolverTask`]s
pub trait SolverBackend {
    /// The repo data associated to a channel and platform combination
    type RepoData<'a>: SolverRepoData<'a>;

    /// Resolve the dependencies and return the [`RepoDataRecord`]s that should be present in the
    /// environment.
    fn solve<
        'a,
        R: IntoRepoData<'a, Self::RepoData<'a>>,
        TAvailablePackagesIterator: IntoIterator<Item = R>,
    >(
        &mut self,
        task: SolverTask<TAvailablePackagesIterator>,
    ) -> Result<Vec<RepoDataRecord>, SolveError>;
}
