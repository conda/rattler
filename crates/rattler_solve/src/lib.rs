//! `rattler_solve` is a crate that provides functionality to solve Conda environments. It currently
//! exposes the functionality through the [`SolverImpl::solve`] function.

#![deny(missing_docs)]

#[cfg(feature = "libsolv_c")]
pub mod libsolv_c;
#[cfg(feature = "resolvo")]
pub mod resolvo;

use rattler_conda_types::{GenericVirtualPackage, MatchSpec, RepoDataRecord};
use std::fmt;

/// Represents a solver implementation, capable of solving [`SolverTask`]s
pub trait SolverImpl {
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

/// Represents an error when solving the dependencies for a given environment
#[derive(thiserror::Error, Debug)]
pub enum SolveError {
    /// There is no set of dependencies that satisfies the requirements
    Unsolvable(Vec<String>),

    /// The solver backend returned operations that we dont know how to install.
    /// Each string is a somewhat user-friendly representation of which operation was not recognized
    /// and can be used for error reporting
    UnsupportedOperations(Vec<String>),

    /// Error when converting matchspec
    #[error(transparent)]
    ParseMatchSpecError(#[from] rattler_conda_types::ParseMatchSpecError),
}

impl fmt::Display for SolveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SolveError::Unsolvable(operations) => {
                write!(
                    f,
                    "Cannot solve the request because of: {}",
                    operations.join(", ")
                )
            }
            SolveError::UnsupportedOperations(operations) => {
                write!(f, "Unsupported operations: {}", operations.join(", "))
            }
            SolveError::ParseMatchSpecError(e) => {
                write!(f, "Error parsing match spec: {}", e)
            }
        }
    }
}

/// Represents a dependency resolution task, to be solved by one of the backends (currently only
/// libsolv is supported)
pub struct SolverTask<TAvailablePackagesIterator> {
    /// An iterator over all available packages
    pub available_packages: TAvailablePackagesIterator,

    /// Records of packages that are previously selected.
    ///
    /// If the solver encounters multiple variants of a single package (identified by its name), it
    /// will sort the records and select the best possible version. However, if there exists a
    /// locked version it will prefer that variant instead. This is useful to reduce the number of
    /// packages that are updated when installing new packages.
    ///
    /// Usually you add the currently installed packages or packages from a lock-file here.
    pub locked_packages: Vec<RepoDataRecord>,

    /// Records of packages that are previously selected and CANNOT be changed.
    ///
    /// If the solver encounters multiple variants of a single package (identified by its name), it
    /// will sort the records and select the best possible version. However, if there is a variant
    /// available in the `pinned_packages` field it will always select that version no matter what
    /// even if that means other packages have to be downgraded.
    pub pinned_packages: Vec<RepoDataRecord>,

    /// Virtual packages considered active
    pub virtual_packages: Vec<GenericVirtualPackage>,

    /// The specs we want to solve
    pub specs: Vec<MatchSpec>,
}

/// A representation of a collection of [`RepoDataRecord`] usable by a [`SolverImpl`]
/// implementation.
///
/// Some solvers might be able to cache the collection between different runs of the solver which
/// could potentially eliminate some overhead. This trait enables creating a representation of the
/// repodata that is most suitable for a specific backend.
///
/// Some solvers may add additional functionality to their specific implementation that enables
/// caching the repodata to disk in an efficient way (see [`crate::libsolv_c::RepoData`] for
/// an example).
pub trait SolverRepoData<'a>: FromIterator<&'a RepoDataRecord> {}

/// Defines the ability to convert a type into [`SolverRepoData`].
pub trait IntoRepoData<'a, S: SolverRepoData<'a>> {
    /// Converts this instance into an instance of [`SolverRepoData`] which is consumable by a
    /// specific [`SolverImpl`] implementation.
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
