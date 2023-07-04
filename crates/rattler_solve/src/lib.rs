#![deny(missing_docs)]

//! `rattler_solve` is a crate that provides functionality to solve Conda environments. It currently
//! exposes the functionality through the [`SolverBackend::solve`] function.

#[cfg(feature = "libsolv-sys")]
mod libsolv;
#[cfg(feature = "libsolv_rs")]
mod libsolv_rs;
mod solver_backend;

#[cfg(feature = "libsolv_rs")]
pub use crate::libsolv_rs::{LibsolvRsBackend, LibsolvRsRepoData};
#[cfg(feature = "libsolv-sys")]
pub use libsolv::{
    cache_repodata as cache_libsolv_repodata, LibcByteSlice, LibsolvBackend, LibsolvRepoData,
};
pub use solver_backend::SolverBackend;
use std::fmt;

use rattler_conda_types::GenericVirtualPackage;
use rattler_conda_types::{MatchSpec, RepoDataRecord};

/// Represents an error when solving the dependencies for a given environment
#[derive(thiserror::Error, Debug)]
pub enum SolveError {
    /// There is no set of dependencies that satisfies the requirements
    Unsolvable(Vec<String>),

    /// The solver backend returned operations that we dont know how to install.
    /// Each string is a somewhat user-friendly representation of which operation was not recognized
    /// and can be used for error reporting
    UnsupportedOperations(Vec<String>),
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
