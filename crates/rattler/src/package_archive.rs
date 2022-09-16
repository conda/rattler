use crate::{ChannelConfig, MatchSpec, NoArchType, Version};
use serde::{Deserialize, Deserializer};
use serde_with::{serde_as, DeserializeAs, DisplayFromStr};
use std::fmt;
use std::marker::PhantomData;

#[serde_as]
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
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_track_features"
    )]
    pub track_features: Vec<String>,

    /// The features defined by the package
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<String>,
}

/// Parses the `track_features` in a package record.
pub(crate) fn deserialize_track_features<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
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
