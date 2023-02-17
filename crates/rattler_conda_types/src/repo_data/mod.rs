//! Defines [`RepoData`]. `RepoData` stores information of all packages present in a subdirectory
//! of a channel. It provides indexing functionality.
//!
//! See the [`fetch`] module for functionality to download this information from a
//! [`crate::Channel`].

use std::fmt::{Display, Formatter};

use fxhash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, DisplayFromStr, OneOrMany};

use crate::{NoArchType, Version};

/// [`RepoData`] is an index of package binaries available on in a subdirectory of a Conda channel.
#[derive(Debug, Deserialize, Eq, PartialEq)]
pub struct RepoData {
    #[serde(rename = "repodata_version")]
    pub version: Option<usize>,
    pub info: Option<ChannelInfo>,
    pub packages: FxHashMap<String, PackageRecord>,
    #[serde(default)]
    pub removed: FxHashSet<String>,
}

/// Information about subdirectory of channel in the Conda [`Repodata`]
#[derive(Debug, Deserialize, Eq, PartialEq)]
pub struct ChannelInfo {
    pub subdir: String,
}

/// A single record in the Conda repodata. A single record refers to a single binary distribution
/// of a package on a Conda channel.
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd, Clone)]
pub struct PackageRecord {
    /// The name of the package
    pub name: String,

    /// The version of the package
    #[serde_as(as = "DisplayFromStr")]
    pub version: Version,

    /// The build string of the package
    #[serde(alias = "build_string")]
    pub build: String,

    /// The build number of the package
    pub build_number: usize,

    /// The subdirectory where the package can be found
    #[serde(default)]
    pub subdir: String,

    /// Optionally a MD5 hash of the package archive
    pub md5: Option<String>,

    /// Optionally a MD5 hash of the package archive
    pub sha256: Option<String>,

    /// Optionally the size of the package archive in bytes
    pub size: Option<usize>,

    /// Optionally the architecture the package supports
    pub arch: Option<String>,

    /// Optionally the platform the package supports
    pub platform: Option<String>, // Note that this does not match the [`Platform`] enum..

    /// Specification of packages this package depends on
    #[serde(default)]
    pub depends: Vec<String>,

    /// Additional constraints on packages. `constrains` are different from `depends` in that packages
    /// specified in `depends` must be installed next to this package, whereas packages specified in
    /// `constrains` are not required to be installed, but if they are installed they must follow these
    /// constraints.
    #[serde(default)]
    pub constrains: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "OneOrMany<_>")]
    pub track_features: Vec<String>,

    pub features: Option<String>,

    /// If this package is independent of architecture this field specifies in what way. See
    /// [`NoArchType`] for more information.
    #[serde(skip_serializing_if = "NoArchType::is_none")]
    pub noarch: NoArchType,

    /// The specific license of the package
    pub license: Option<String>,

    /// The license family
    pub license_family: Option<String>,

    /// The UNIX Epoch timestamp when this package was created. Note that sometimes this is specified in
    /// seconds and sometimes in milliseconds.
    pub timestamp: Option<usize>,
    //pub preferred_env: Option<String>,
    //pub date: Option<String>,
    //pub legacy_bz2_md5: Option<String>,
    //pub legacy_bz2_size: Option<usize>,
    //pub package_type: ?
}

impl Display for PackageRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}={}", self.name, self.version, self.build)
    }
}
