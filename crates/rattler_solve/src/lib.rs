#![deny(missing_docs)]

//! `rattler_solve` is a crate that provides functionality to solve Conda environments. It currently
//! exposes the functionality through the [`SolverProblem::solve`] function.

mod libsolv;
mod package_operation;

pub use package_operation::{PackageIdentifier, PackageOperation, PackageOperationKind};

use rattler_conda_types::{MatchSpec, RepoData};

/// Represents an error when solving the dependencies for a given environment
#[derive(thiserror::Error, Debug)]
pub enum SolveError {
    /// There is no set of dependencies that satisfies the requirements
    #[error("unsolvable")]
    Unsolvable,

    /// An error occurred when trying to load the channel and platform's `repodata.json`
    #[error("error adding repodata: {0}")]
    ErrorAddingRepodata(#[source] anyhow::Error),

    /// An error occurred when trying to load information about installed packages to the solver
    #[error("error adding installed packages: {0}")]
    ErrorAddingInstalledPackages(#[source] anyhow::Error),

    /// The solver backend returned operations that have no known mapping to [`PackageOperationKind`]
    #[error("unsupported operations")]
    UnsupportedOperations,
}

/// Represents known information about a single package that is already installed in the Conda
/// environment
#[derive(Debug)]
pub struct InstalledPackage {
    /// The package's name
    pub name: String,
    /// The package's version
    pub version: String,

    // TODO: does it make sense to include the following data?
    /// The package's build string, if present
    pub build_string: Option<String>,
    /// The package's build number, if present
    pub build_number: Option<usize>,
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
#[derive(Debug)]
pub struct SolverProblem<'c> {
    /// All the available channels (and contents) in order of priority
    pub channels: Vec<(String, &'c RepoData)>,

    /// All the packages currently installed, including virtual packages
    pub installed_packages: Vec<InstalledPackage>,

    /// The specs we want to solve
    pub specs: Vec<(MatchSpec, RequestedAction)>,
}

impl<'c> SolverProblem<'c> {
    /// Resolve the dependencies and return the required [`PackageOperation`]s in the order in which
    /// they need to be applied
    pub fn solve(self) -> Result<Vec<PackageOperation>, SolveError> {
        // TODO: support other backends, such as https://github.com/pubgrub-rs/pubgrub
        libsolv::solve(self)
    }
}
