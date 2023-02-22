use rattler_conda_types::RepoDataRecord;

/// Represents an operation that needs to be applied on a package, in order to bring the Conda
/// environment to the desired state
#[derive(Debug, Clone)]
pub struct PackageOperation {
    /// The package to be operated on
    pub package: RepoDataRecord,
    /// The required operation kind
    pub kind: PackageOperationKind,
}

/// Represents the operations that rattler supports on packages
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageOperationKind {
    /// The package should be installed
    Install,
    /// The package should be removed
    Remove,
    /// The package should be reinstalled
    Reinstall,
}
