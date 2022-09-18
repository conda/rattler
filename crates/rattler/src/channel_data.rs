use crate::{RunExports, Version};
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::{serde_as, DeserializeAs, DisplayFromStr, OneOrMany, Same};
use std::collections::HashMap;
use std::marker::PhantomData;
use url::Url;

/// [`ChannelData`] is an index of subdirectories and packages stored within a Channel.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ChannelData {
    /// Version of the format
    pub channeldata_version: u32,

    /// A mapping of all packages in the channel
    pub packages: HashMap<String, ChannelDataPackage>,

    #[serde(default)]
    pub subdirs: Vec<String>,
}

/// Information on a package in a channel.
#[serde_as]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ChannelDataPackage {
    /// True if this package has activation scripts
    #[serde(rename = "activate.d")]
    pub has_activate_scripts: bool,

    /// True if this package has deactivation scripts
    #[serde(rename = "deactivate.d")]
    pub has_deactivate_scripts: bool,

    /// True if this package contains binary files that contain the build prefix.
    pub binary_prefix: bool,

    /// The description of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// URL to the development page of the package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[serde_as(
        deserialize_as = "VecSkipNone<OneOrMany<LossyUrl>>",
        serialize_as = "OneOrMany<Same>"
    )]
    pub dev_url: Vec<Url>,

    /// URL to the documentation of the package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[serde_as(
        deserialize_as = "VecSkipNone<OneOrMany<LossyUrl>>",
        serialize_as = "OneOrMany<Same>"
    )]
    pub doc_url: Vec<Url>,

    /// URL to the homepage of the package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[serde_as(
        deserialize_as = "VecSkipNone<OneOrMany<LossyUrl>>",
        serialize_as = "OneOrMany<Same>"
    )]
    pub home: Vec<Url>,

    /// URL to the latest source code of the package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[serde_as(
        deserialize_as = "VecSkipNone<OneOrMany<LossyUrl>>",
        serialize_as = "OneOrMany<Same>"
    )]
    pub source_url: Vec<Url>,

    /// Package license
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Whether the package has post link scripts
    #[serde(rename = "post_link")]
    pub has_post_link_scripts: bool,

    /// Whether the package has pre link scripts
    #[serde(rename = "pre_link")]
    pub has_pre_link_scripts: bool,

    /// Whether the package has pre unlink scripts
    #[serde(rename = "pre_unlink")]
    pub has_pre_unlink_scripts: bool,

    /// Any run_exports contained within the package.
    #[serde_as(as = "HashMap<DisplayFromStr, _>")]
    pub run_exports: HashMap<Version, RunExports>,

    /// Which architectures does the package support
    pub subdirs: Vec<String>,

    /// The summary description of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// True if this package contains text files that contain the build prefix.
    pub text_prefix: bool,

    /// Last update time
    pub timestamp: usize,

    /// Latest version
    #[serde_as(as = "DisplayFromStr")]
    pub version: Version,
}

/// Deserialize a sequence into `Vec<T>` but filter `None` values.
pub struct VecSkipNone<T>(PhantomData<T>);

impl<'de, T, I> DeserializeAs<'de, Vec<T>> for VecSkipNone<I>
where
    I: DeserializeAs<'de, Vec<Option<T>>>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(I::deserialize_as(deserializer)?
            .into_iter()
            .filter_map(std::convert::identity)
            .collect())
    }
}

/// A helper type parser that tries to parse Urls that could be malformed.
struct LossyUrl;

impl<'de> DeserializeAs<'de, Option<Url>> for LossyUrl {
    fn deserialize_as<D>(deserializer: D) -> Result<Option<Url>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str = match Option::<String>::deserialize(deserializer)? {
            Some(url) => url,
            None => return Ok(None),
        };
        let url = match Url::parse(&str) {
            Ok(url) => url,
            Err(e) => {
                tracing::warn!("unable to parse '{}' as an URL: {e}. Skipping...", str);
                return Ok(None);
            }
        };
        Ok(Some(url))
    }
}
