mod libsolv;
mod package_operation;

pub use package_operation::{PackageIdentifier, PackageOperation, PackageOperationKind};

use rattler_conda_types::{MatchSpec, RepoData};

#[derive(thiserror::Error, Debug)]
pub enum SolveError {
    #[error("unsolvable")]
    Unsolvable,

    #[error("error adding repodata: {0}")]
    ErrorAddingRepodata(#[source] anyhow::Error),

    #[error("error adding installed packages: {0}")]
    ErrorAddingInstalledPackages(#[source] anyhow::Error),

    #[error("unsupported operations")]
    UnsupportedOperations,
}

#[derive(Debug)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,

    // TODO: does it make sense to include the following data?
    pub build_string: Option<String>,
    pub build_number: Option<usize>,
}

#[derive(Debug)]
pub struct SolverProblem<'c> {
    /// All the available channels (and contents) in order of priority
    pub channels: Vec<(String, &'c RepoData)>,

    /// All the packages currently installed, including virtual packages
    pub installed_packages: Vec<InstalledPackage>,

    /// The specs we want to solve
    pub specs: Vec<(MatchSpec, RequestedAction)>,
}

#[derive(Debug, Copy, Clone)]
pub enum RequestedAction {
    Install,
    Remove,
    Update,
}

impl<'c> SolverProblem<'c> {
    pub fn solve(self) -> Result<Vec<PackageOperation>, SolveError> {
        // TODO: support other backends, such as https://github.com/pubgrub-rs/pubgrub
        libsolv::solve(self)
    }
}
