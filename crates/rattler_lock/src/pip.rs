use crate::PackageHashes;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};
use std::collections::HashSet;
use url::Url;

/// A pinned Pip package
#[serde_as]
#[skip_serializing_none]
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct PipLockedDependency {
    /// A list of dependencies on other packages that the wheel listed.
    #[serde(default, alias = "dependencies")]
    #[serde_as(deserialize_as = "crate::utils::serde::Pep440MapOrVec")]
    pub requires_dist: Vec<String>,

    /// The python version that this package requires.
    pub requires_python: Option<String>,

    /// A list of extras that are selected
    #[serde(default)]
    pub extras: HashSet<String>,

    /// The URL that points to where the artifact can be downloaded from.
    pub url: Url,

    /// Hashes of the file pointed to by `url`.
    pub hash: Option<PackageHashes>,

    /// ???
    pub source: Option<Url>,

    /// Build string
    pub build: Option<String>,
}
