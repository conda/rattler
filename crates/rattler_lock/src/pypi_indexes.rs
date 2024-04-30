use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::path::{Path, PathBuf};
use url::Url;

/// Defines the pypi indexes that were used during solving.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PypiIndexes {
    /// The indexes used to resolve the dependencies.
    pub indexes: Vec<Url>,

    /// Flat indexes also called `--find-links` in pip
    /// These are flat listings of distributions
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        with = "serde_yaml::with::singleton_map_recursive"
    )]
    pub find_links: Vec<FindLinksUrlOrPath>,
}

/// A flat index is a static source of
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum FindLinksUrlOrPath {
    /// Can be a path to a directory or a file containing the flat index
    Path(PathBuf),

    /// Can be a URL to a flat index
    Url(Url),
}

impl FindLinksUrlOrPath {
    /// Returns the URL if it is a URL
    pub fn as_url(&self) -> Option<&Url> {
        match self {
            Self::Path(_) => None,
            Self::Url(url) => Some(url),
        }
    }

    /// Returns the path if it is a path
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path),
            Self::Url(_) => None,
        }
    }
}
