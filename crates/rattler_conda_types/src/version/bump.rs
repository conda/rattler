use std::fmt;

/// VersionBumpType is used to specify the type of bump to perform on a version.
pub enum VersionBumpType {
    /// Bump the major version number.
    Major,
    /// Bump the minor version number.
    Minor,
    /// Bump the patch version number.
    Patch,
    /// Bump the last  version number.
    Last,
}

/// VersionBumpError is used to specify the type of error that occurred when bumping a version.
#[derive(Debug, Clone)]
pub struct VersionBumpError {
    /// The reason for the error.
    pub reason: String,
}

impl fmt::Display for VersionBumpError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Error bumping the version: {}", self.reason)
    }
}
