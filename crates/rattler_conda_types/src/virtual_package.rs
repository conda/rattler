use crate::Version;

/// A `GenericVirtualPackage` is a Conda package description that contains a `name` and a
/// `version` and a `build_string`.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct GenericVirtualPackage {
    /// The name of the package
    pub name: String,

    /// The version of the package
    pub version: Version,

    /// The build identifier of the package.
    pub build_string: String,
}
