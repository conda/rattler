use crate::PackageHashes;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};
use std::cmp::Ordering;
use std::collections::HashSet;
use url::Url;

/// A pinned Pypi package
#[serde_as]
#[skip_serializing_none]
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Hash)]
pub struct PypiPackageData {
    /// The name of the package.
    pub name: String,

    /// The version of the package.
    pub version: pep440_rs::Version,

    /// A list of dependencies on other packages that the wheel listed.
    #[serde(default, alias = "dependencies", skip_serializing_if = "Vec::is_empty")]
    pub requires_dist: Vec<String>,

    /// The python version that this package requires.
    pub requires_python: Option<String>,

    /// The URL that points to where the artifact can be downloaded from.
    pub url: Url,

    /// Hashes of the file pointed to by `url`.
    #[serde(flatten)]
    pub hash: Option<PackageHashes>,

    /// ???
    pub source: Option<Url>,

    /// Build string
    pub build: Option<String>,
}

/// Additional runtime configuration of a package. Multiple environments/platforms might refer to
/// the same pypi package but with different extras enabled.
#[derive(Clone, Debug)]
pub struct PyPiRuntimeConfiguration {
    /// The extras enabled for the package
    pub extras: HashSet<String>,
}

impl PartialOrd for PypiPackageData {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PypiPackageData {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.version.cmp(&other.version))
    }
}
