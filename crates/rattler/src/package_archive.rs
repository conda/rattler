use crate::{ChannelConfig, MatchSpec, NoArchType, Version};
use serde::{Deserialize, Deserializer};
use serde_with::{serde_as, skip_serializing_none, DeserializeAs, DisplayFromStr, OneOrMany};

#[serde_as]
#[skip_serializing_none]
#[derive(Clone, Debug, Deserialize)]
pub struct Index {
    /// The architecture of the package
    pub arch: Option<String>,

    /// The noarch type of the package
    pub noarch: NoArchType,

    /// The build string which unique identifies the package
    pub build: String,

    /// The build number
    pub build_number: usize,

    /// License of the package
    pub license: Option<String>,

    /// The license family
    pub license_family: Option<String>,

    /// The name of the package. This does not include the build string, etc.
    pub name: String,

    /// The subdirectory in the channel (usually the architecture) in which the package is stored.
    pub subdir: String,

    /// The timestamp on which this package as created
    pub timestamp: Option<usize>,

    /// The version of the package
    #[serde_as(as = "DisplayFromStr")]
    pub version: Version,

    /// The dependencies of the package
    #[serde_as(as = "Vec<MatchSpecStr>")]
    pub depends: Vec<MatchSpec>,

    /// Any tracked features
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "OneOrMany<_>")]
    pub track_features: Vec<String>,

    /// The features defined by the package
    pub features: Option<String>,
}

pub(crate) struct MatchSpecStr;

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

/// All supported package archives supported by Rattler.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum PackageArchiveFormat {
    TarBz2,
    TarZst,
    Conda,
}

impl PackageArchiveFormat {
    /// Determine the format of an archive based on the file name of a package. Returns the format
    /// and the original name of the package (without archive extension).
    pub fn from_file_name(file_name: &str) -> Option<(&str, Self)> {
        if let Some(name) = file_name.strip_suffix(".tar.bz2") {
            Some((name, PackageArchiveFormat::TarBz2))
        } else if let Some(name) = file_name.strip_suffix(".conda") {
            Some((name, PackageArchiveFormat::Conda))
        } else if let Some(name) = file_name.strip_suffix(".tar.zst") {
            Some((name, PackageArchiveFormat::TarZst))
        } else {
            None
        }
    }
}
