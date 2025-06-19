use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::{Config, MergeError, ValidationError};

#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RepodataChannelConfig {
    /// Disable JLAP compression for repodata.
    #[serde(alias = "disable_jlap")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_jlap: Option<bool>,

    /// Disable bzip2 compression for repodata.
    #[serde(alias = "disable_bzip2")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_bzip2: Option<bool>,

    /// Disable zstd compression for repodata.
    #[serde(alias = "disable_zstd")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_zstd: Option<bool>,

    /// Disable the use of sharded repodata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_sharded: Option<bool>,
}

impl RepodataChannelConfig {
    pub fn is_empty(&self) -> bool {
        self.disable_jlap.is_none()
            && self.disable_bzip2.is_none()
            && self.disable_zstd.is_none()
            && self.disable_sharded.is_none()
    }

    pub fn merge(&self, other: Self) -> Self {
        Self {
            disable_jlap: self.disable_jlap.or(other.disable_jlap),
            disable_zstd: self.disable_zstd.or(other.disable_zstd),
            disable_bzip2: self.disable_bzip2.or(other.disable_bzip2),
            disable_sharded: self.disable_sharded.or(other.disable_sharded),
        }
    }
}

// impl From<RepodataChannelConfig> for SourceConfig {
//     fn from(value: RepodataChannelConfig) -> Self {
//         SourceConfig {
//             jlap_enabled: !value.disable_jlap.unwrap_or(false),
//             zstd_enabled: !value.disable_zstd.unwrap_or(false),
//             bz2_enabled: !value.disable_bzip2.unwrap_or(false),
//             sharded_enabled: !value.disable_sharded.unwrap_or(false),
//             cache_action: Default::default(),
//         }
//     }
// }

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct RepodataConfig {
    /// Default configuration for all channels.
    #[serde(flatten)]
    pub default: RepodataChannelConfig,

    /// Per-channel configuration for repodata.
    #[serde(flatten)]
    pub per_channel: HashMap<Url, RepodataChannelConfig>,
}

impl RepodataConfig {
    pub fn is_empty(&self) -> bool {
        self.default.is_empty() && self.per_channel.is_empty()
    }
}

impl Config for RepodataConfig {
    fn get_extension_name(&self) -> String {
        "repodata".to_string()
    }

    /// Merge the given `RepodataConfig` into the current one.
    /// The `other` configuration should take priority over the current one.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        // Make `other` mutable to allow for moving the values out of it.
        let mut merged = self.clone();
        merged.default = merged.default.merge(other.default.clone());
        for (url, config) in &other.per_channel {
            merged
                .per_channel
                .entry(url.clone())
                .and_modify(|existing| *existing = existing.merge(config.clone()))
                .or_insert_with(|| config.clone());
        }
        Ok(merged)
    }

    fn validate(&self) -> Result<(), ValidationError> {
        if self.default.is_empty() && self.per_channel.is_empty() {
            return Err(ValidationError::InvalidValue(
                "repodata".to_string(),
                "RepodataConfig must not be empty".to_string(),
            ));
        }

        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec!["default".to_string(), "per-channel".to_string()]
    }
}
