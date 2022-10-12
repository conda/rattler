//! Defines [`RepoData`]. `RepoData` stores information of all packages present in a subdirectory
//! of a channel. It provides indexing functionality.
//!
//! See the [`fetch`] module for functionality to download this information from a
//! [`crate::Channel`].

use std::fmt::{Display, Formatter};

use fxhash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use serde_with::{serde_as, skip_serializing_none, DisplayFromStr, OneOrMany};

use crate::{NoArchType, Version};

pub mod fetch;

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
#[derive(Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Clone)]
pub struct PackageRecord {
    pub name: String,

    #[serde_as(as = "DisplayFromStr")]
    pub version: Version,

    #[serde(alias = "build_string")]
    pub build: String,
    pub build_number: usize,

    //pub channel: Channel,
    #[serde(default)]
    pub subdir: String,
    #[serde(default, rename = "fn")]
    pub filename: Option<String>,

    pub md5: Option<String>,
    //pub legacy_bz2_md5: Option<String>,
    //pub legacy_bz2_size: Option<usize>,
    pub sha256: Option<String>,

    pub arch: Option<String>,
    pub platform: Option<String>, // Note that this does not match the [`Platform`] enum..

    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub constrains: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "OneOrMany<_>")]
    pub track_features: Vec<String>,

    pub features: Option<String>,

    pub noarch: NoArchType,

    pub preferred_env: Option<String>,

    pub license: Option<String>,
    pub license_family: Option<String>,

    // pub package_type: ?
    pub timestamp: Option<usize>,

    pub date: Option<String>,

    pub size: Option<usize>,
}

impl Display for PackageRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}={}", self.name, self.version, self.build)
    }
}
