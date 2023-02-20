/// Represents an operation that needs to be applied on a package, in order to bring the Conda
/// environment to the desired state
#[derive(Debug, Clone)]
pub struct PackageOperation {
    /// The package to be operated on
    pub package: PackageIdentifier,
    /// The required operation kind
    pub kind: PackageOperationKind,
}

/// Represents a package (installed or not) that will be modified
#[derive(Debug, Clone, serde::Serialize)]
pub struct PackageIdentifier {
    /// The package's name
    pub name: String,
    /// The package's version
    pub version: String,
    /// The package's location
    pub location: Option<String>,
    /// The package's channel
    pub channel: String,
    /// The package's build string, if known
    pub build_string: Option<String>,
    /// The package's build number, if known
    pub build_number: Option<usize>,
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
