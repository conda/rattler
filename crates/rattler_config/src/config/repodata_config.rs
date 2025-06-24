use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::{Config, MergeError, ValidationError};
#[cfg(feature = "edit")]
use crate::edit::ConfigEditError;

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

    #[cfg(feature = "edit")]
    fn set(
        &mut self,
        key: &str,
        value: Option<String>,
    ) -> Result<(), crate::config::ConfigEditError> {
        if key == "repodata-config" {
            *self = value
                .map(|v| {
                    serde_json::de::from_str(&v).map_err(|e| ConfigEditError::JsonParseError {
                        key: key.to_string(),
                        source: e,
                    })
                })
                .transpose()?
                .unwrap_or_default();
            return Ok(());
        } else if !key.starts_with("repodata-config.") {
            return Err(ConfigEditError::UnknownKeyInner {
                key: key.to_string(),
            });
        }

        let subkey = key.strip_prefix("repodata-config.").unwrap();
        match subkey {
            "disable-jlap" => {
                self.default.disable_jlap = value
                    .map(|v| {
                        v.parse().map_err(|e| ConfigEditError::BoolParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?;
            }
            "disable-bzip2" => {
                self.default.disable_bzip2 = value
                    .map(|v| {
                        v.parse().map_err(|e| ConfigEditError::BoolParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?;
            }
            "disable-zstd" => {
                self.default.disable_zstd = value
                    .map(|v| {
                        v.parse().map_err(|e| ConfigEditError::BoolParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?;
            }
            "disable-sharded" => {
                self.default.disable_sharded = value
                    .map(|v| {
                        v.parse().map_err(|e| ConfigEditError::BoolParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?;
            }
            _ => {
                return Err(ConfigEditError::UnknownKeyInner {
                    key: key.to_string(),
                })
            }
        }
        Ok(())
    }
}
