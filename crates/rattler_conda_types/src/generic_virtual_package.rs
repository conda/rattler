use crate::Version;
use std::fmt::{Display, Formatter};

/// A `GenericVirtualPackage` is a Conda package description that contains a `name` and a
/// `version` and a `build_string`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct GenericVirtualPackage {
    /// The name of the package
    pub name: String,

    /// The version of the package
    pub version: Version,

    /// The build identifier of the package.
    pub build_string: String,
}

impl Display for GenericVirtualPackage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}={}", &self.name, &self.version, &self.build_string)
    }
}
