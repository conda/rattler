use serde::{Deserialize, Serialize};

/// This is metadata about the package version that is contained in `.conda` packages only, in the
/// outer zip archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageMetadata {
    /// The version of the conda package format. This is currently always 2.
    pub conda_pkg_format_version: u64,
}

impl Default for PackageMetadata {
    fn default() -> Self {
        Self {
            conda_pkg_format_version: 2,
        }
    }
}
