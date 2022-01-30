use crate::{Platform, VersionOrder};
use fxhash::{FxHashMap, FxHashSet};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, Copy)]
pub enum NoArchType {
    Generic,
    Python,
}

/// [`RepoData`] is an index of package binaries available on in a subdirectory of a Conda channel.
#[derive(Debug, Deserialize)]
pub struct RepoData {
    #[serde(rename = "repodata_version")]
    pub version: Option<usize>,
    pub info: Option<ChannelInfo>,
    pub packages: FxHashMap<String, PackageRecord>,
    #[serde(default)]
    pub removed: FxHashSet<String>,
}

/// Information about subdirectory of channel in the Conda [`Repodata`]
#[derive(Debug, Deserialize)]
pub struct ChannelInfo {
    pub subdir: String,
}

/// A single record in the Conda repodata. A single record refers to a single binary distribution
/// of a package on a Conda channel.
#[derive(Debug, Deserialize)]
pub struct PackageRecord {
    pub name: String,
    #[serde(deserialize_with = "version_from_str")]
    pub version: VersionOrder,
    #[serde(alias="build_string")]
    pub build: String,
    pub build_number: usize,

    //pub channel: Channel,
    pub subdir: String,
    //pub filename: String

    pub md5: Option<String>,
    //pub legacy_bz2_md5: Option<String>,
    //pub legacy_bz2_size: Option<usize>,

    pub sha256: Option<String>,

    pub arch: Option<String>,
    pub platform: Option<Platform>,

    #[serde(default, skip_serializing_if="Vec::is_empty")]
    pub depends: Vec<String>,
    #[serde(default, skip_serializing_if="Vec::is_empty")]
    pub constrains: Vec<String>,

    #[serde(default, skip_serializing_if="Vec::is_empty")]
    pub track_features: Vec<String>,
    #[serde(default, skip_serializing_if="Vec::is_empty")]
    pub features: Vec<String>,

    #[serde(skip_serializing_if="Option::is_none")]
    pub noarch: Option<NoArchType>,

    #[serde(skip_serializing_if="Option::is_none")]
    pub preferred_env: Option<String>,

    #[serde(skip_serializing_if="Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub license_family: Option<String>,

    // pub package_type: ?

    #[serde(skip_serializing_if="Option::is_none")]
    pub timestamp: Option<usize>,

    #[serde(skip_serializing_if="Option::is_none")]
    pub date: Option<String>,

    #[serde(skip_serializing_if="Option::is_none")]
    pub size: Option<usize>,

}

/// Parses a version from a string
fn version_from_str<'de, D>(deserializer: D) -> Result<VersionOrder, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer)?
        .parse()
        .map_err(serde::de::Error::custom)
}
