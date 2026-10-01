use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::Config;

#[derive(Default, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct S3OptionsMap(pub IndexMap<String, S3Options>);

impl S3OptionsMap {
    /// Returns `true` if no S3 buckets are configured.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// How to address an S3 bucket.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum S3AddressingStyle {
    /// Address the bucket as a virtual host, e.g.
    /// <https://bucket-name.s3.us-east-1.amazonaws.com>.
    #[default]
    VirtualHost,

    /// Address the bucket through the path, e.g.
    /// <https://s3.us-east-1.amazonaws.com/bucket-name>.
    Path,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct S3Options {
    /// S3 endpoint URL
    pub endpoint_url: Url,

    /// The name of the S3 region
    pub region: String,

    /// How to address the bucket
    #[serde(default)]
    pub addressing_style: S3AddressingStyle,
}

impl Config for S3OptionsMap {
    fn is_default(&self) -> bool {
        self.0.is_empty()
    }

    fn merge_config(self, other: &Self) -> Result<Self, super::MergeError> {
        // Merge the two S3OptionMaps, overwriting existing keys
        let mut merged = self.0.clone();
        for (key, value) in &other.0 {
            merged.insert(key.clone(), value.clone());
        }
        Ok(S3OptionsMap(merged))
    }

    fn validate(&self) -> Result<(), super::ValidationError> {
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        self.0.keys().map(ToString::to_string).collect()
    }
}
