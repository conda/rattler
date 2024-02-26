use crate::PackageHashes;
use pep440_rs::VersionSpecifiers;
use pep508_rs::Requirement;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};
use std::cmp::Ordering;
use std::collections::BTreeSet;
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

    /// The URL that points to where the artifact can be downloaded from.
    pub url: Url,

    /// Hashes of the file pointed to by `url`.
    #[serde(flatten)]
    pub hash: Option<PackageHashes>,

    /// A list of dependencies on other packages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_dist: Vec<Requirement>,

    /// The python version that this package requires.
    pub requires_python: Option<VersionSpecifiers>,
}

/// Additional runtime configuration of a package. Multiple environments/platforms might refer to
/// the same pypi package but with different extras enabled.
#[derive(Clone, Debug, Default)]
pub struct PypiPackageEnvironmentData {
    /// The extras enabled for the package. Note that the order doesn't matter here but it does matter for serialization.
    pub extras: BTreeSet<String>,
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

impl PypiPackageData {
    /// Returns true if this package satisfies the given `spec`.
    pub fn satisfies(&self, spec: &Requirement) -> bool {
        // Check if the name matches
        if spec.name != self.name {
            return false;
        }

        // Check if the version of the requirement matches
        match &spec.version_or_url {
            None => {}
            Some(pep508_rs::VersionOrUrl::Url(_)) => return false,
            Some(pep508_rs::VersionOrUrl::VersionSpecifier(spec)) => {
                if !spec.contains(&self.version) {
                    return false;
                }
            }
        }

        true
    }
}
