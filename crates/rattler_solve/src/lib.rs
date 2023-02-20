#![deny(missing_docs)]

//! `rattler_solve` is a crate that provides functionality to solve Conda environments. It currently
//! exposes the functionality through the [`SolverProblem::solve`] function.

mod libsolv;
mod package_operation;

use std::ffi::NulError;
pub use package_operation::{PackageIdentifier, PackageOperation, PackageOperationKind};

use rattler_conda_types::{MatchSpec, PrefixRecord, RepoDataRecord};
use rattler_virtual_packages::GenericVirtualPackage;

/// Represents an error when solving the dependencies for a given environment
#[derive(thiserror::Error, Debug)]
pub enum SolveError {
    /// There is no set of dependencies that satisfies the requirements
    #[error("unsolvable")]
    Unsolvable,

    /// An error occurred when trying to load the channel and platform's `repodata.json`
    #[error("error adding repodata: {0}")]
    ErrorAddingRepodata(#[source] NulError),

    /// An error occurred when trying to load information about installed packages to the solver
    #[error("error adding installed packages: {0}")]
    ErrorAddingInstalledPackages(#[source] NulError),

    /// The solver backend returned operations that have no known mapping to [`PackageOperationKind`]
    #[error("unsupported operations")]
    UnsupportedOperations,
}

/// Represents the action that we want to perform on a given package, so the solver can take it into
/// account (e.g. specifying [`RequestedAction::Install`] for a package that has already been
/// installed will result in no operations, but specifying [`RequestedAction::Update`] will generate
/// the necessary operations to update the package to a newer version if it exists and the update is
/// compatible with the rest of the environment).
#[derive(Debug, Copy, Clone)]
pub enum RequestedAction {
    /// The package is being installed
    Install,
    /// The package is being removed
    Remove,
    /// The package is being updated
    Update,
}

/// Represents a dependency resolution problem, to be solved by one of the backends (currently only
/// libsolv is supported)
pub struct SolverProblem {
    /// All the available packages
    pub available_packages: Vec<Vec<RepoDataRecord>>,

    /// All the packages currently installed
    pub installed_packages: Vec<PrefixRecord>,

    /// Virtual packages considered active
    pub virtual_packages: Vec<GenericVirtualPackage>,

    /// The specs we want to solve
    pub specs: Vec<(MatchSpec, RequestedAction)>,
}

impl SolverProblem {
    /// Resolve the dependencies and return the required [`PackageOperation`]s in the order in which
    /// they need to be applied
    pub fn solve(self) -> Result<Vec<PackageOperation>, SolveError> {
        // TODO: support other backends, such as https://github.com/pubgrub-rs/pubgrub
        libsolv::solve(self)
    }
}
