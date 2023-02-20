/// Enumeration of all operations on packages
#[derive(Debug, Clone)]
pub struct PackageOperation {
    pub package: PackageIdentifier,
    pub kind: PackageOperationKind,
}

/// A package identifier, used for deciding what to install/remove and where
#[derive(Debug, Clone, serde::Serialize)]
pub struct PackageIdentifier {
    pub name: String,
    pub version: String,
    pub location: Option<String>,
    pub channel: String,
    pub build_string: Option<String>,
    pub build_number: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageOperationKind {
    Install,
    Remove,
    Reinstall,
}
