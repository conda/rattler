//! Defines [`RepoData`]. `RepoData` stores information of all packages present in a subdirectory
//! of a channel. It provides indexing functionality.
//!
//! See the [`fetch`] module for functionality to download this information from a
//! [`crate::Channel`].

use std::{
    fmt::{self, Display, Formatter},
    marker::PhantomData,
};

use fxhash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Deserializer};
use serde_with::{serde_as, DeserializeAs};

use crate::{ChannelConfig, MatchSpec, Version};

pub mod fetch;

/// Noarch packages are packages that are not architecture specific and therefore only have to be
/// built once. Noarch packages are either generic or Python.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum NoArchType {
    /// Noarch generic packages allow users to distribute docs, datasets, and source code in conda
    /// packages. This differs from [`GenericV2`] by how it is stored in the repodata (old-format vs
    /// new-format)
    GenericV1,

    /// Noarch generic packages allow users to distribute docs, datasets, and source code in conda
    /// packages. This differs from [`GenericV2`] by how it is stored in the repodata (new-format vs
    /// old-format)
    GenericV2,

    /// A noarch python package is a python package without any precompiled python files (`.pyc` or
    /// `__pycache__`). Normally these files are bundled with the package. However, these files are
    /// tied to a specific version of Python and must therefor be generated for every target
    /// platform and architecture. This complicates the build process.
    ///
    /// For noarch python packages these files are generated when installing the package by invoking
    /// the compilation process through the python binary that is installed in the same environment.
    ///
    /// This introductory blog post highlights some of specific of noarch python packages:
    /// https://www.anaconda.com/blog/condas-new-noarch-packages
    ///
    /// Or read the docs for more information:
    /// https://docs.conda.io/projects/conda/en/latest/user-guide/concepts/packages.html#noarch-python
    Python,
}

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
#[derive(Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Clone)]
pub struct PackageRecord {
    pub name: String,
    #[serde(deserialize_with = "version_from_str")]
    pub version: Version,
    #[serde(alias = "build_string")]
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
    pub platform: Option<String>, // Note that this does not match the [`Platform`] enum..

    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub constrains: Vec<String>,

    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_track_features"
    )]
    pub track_features: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<String>,

    #[serde(deserialize_with = "deserialize_no_arch", default)]
    pub noarch: Option<NoArchType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_env: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_family: Option<String>,

    // pub package_type: ?
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>,
}

impl Display for PackageRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}={}", self.name, self.version, self.build)
    }
}

/// Parses a version from a string
fn version_from_str<'de, D>(deserializer: D) -> Result<Version, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer)?
        .parse()
        .map_err(serde::de::Error::custom)
}

fn _matchspec_from_str<'de, D>(deserializer: D) -> Result<MatchSpec, D::Error>
where
    D: serde::Deserializer<'de>,
{
    MatchSpec::from_str(
        Deserialize::deserialize(deserializer)?,
        &ChannelConfig::default(),
    )
    .map_err(serde::de::Error::custom)
}

struct MatchSpecStr;

impl<'de> DeserializeAs<'de, MatchSpec> for MatchSpecStr {
    fn deserialize_as<D>(deserializer: D) -> Result<MatchSpec, D::Error>
    where
        D: Deserializer<'de>,
    {
        MatchSpec::from_str(
            Deserialize::deserialize(deserializer)?,
            &ChannelConfig::default(),
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Deserializer the parse the `noarch` field in conda package data.
fn deserialize_no_arch<'de, D>(deserializer: D) -> Result<Option<NoArchType>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Clone, Debug, Deserialize)]
    #[serde(untagged)]
    enum NoArchSerde {
        OldFormat(bool),
        NewFormat(NoArchTypeSerde),
    }

    #[derive(Clone, Debug, Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum NoArchTypeSerde {
        Python,
        Generic,
    }

    let value = Option::<NoArchSerde>::deserialize(deserializer)?;
    Ok(value.and_then(|value| match value {
        NoArchSerde::OldFormat(true) => Some(NoArchType::GenericV1),
        NoArchSerde::OldFormat(false) => None,
        NoArchSerde::NewFormat(NoArchTypeSerde::Python) => Some(NoArchType::Python),
        NoArchSerde::NewFormat(NoArchTypeSerde::Generic) => Some(NoArchType::GenericV2),
    }))
}

fn deserialize_track_features<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct StringOrVec(PhantomData<Vec<String>>);

    impl<'de> de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string or list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<S>(self, visitor: S) -> Result<Self::Value, S::Error>
        where
            S: de::SeqAccess<'de>,
        {
            Deserialize::deserialize(de::value::SeqAccessDeserializer::new(visitor))
        }
    }

    deserializer.deserialize_any(StringOrVec(PhantomData))
}
