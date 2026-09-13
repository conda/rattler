use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::{Config, MergeError, ValidationError};

#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
// Note: no `deny_unknown_fields` — downstream configs (e.g. pixi) need
// to silently tolerate deprecated per-channel keys like `disable-jlap`,
// surfacing them as `serde_ignored` warnings rather than hard errors.
pub struct RepodataChannelConfig {
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
        self.disable_bzip2.is_none()
            && self.disable_zstd.is_none()
            && self.disable_sharded.is_none()
    }

    pub fn merge(&self, other: Self) -> Self {
        Self {
            disable_zstd: self.disable_zstd.or(other.disable_zstd),
            disable_bzip2: self.disable_bzip2.or(other.disable_bzip2),
            disable_sharded: self.disable_sharded.or(other.disable_sharded),
        }
    }
}

#[derive(Clone, Default, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct RepodataConfig {
    /// Default configuration for all channels.
    #[serde(flatten)]
    pub default: RepodataChannelConfig,

    /// Per-channel configuration for repodata.
    #[serde(flatten)]
    pub per_channel: HashMap<Url, RepodataChannelConfig>,
}

// Hand-rolled tolerant deserializer.
//
// The default `#[serde(flatten)]`-based deserialization here would
// dispatch every TOML key either into `RepodataChannelConfig` (known
// keys) or into the per-channel `HashMap<Url, _>` (which would fail
// for any non-URL key). That makes deprecated keys like `disable-jlap`
// a hard error.
//
// Instead: recognise the known control keys (with snake_case aliases),
// route URL-parseable keys to `per_channel`, and silently consume
// anything else as `IgnoredAny`. Downstreams that wrap the deserializer
// in `serde_ignored` will see the unknown keys reported as ignored
// fields, which lets them surface deprecation warnings without
// breaking the config load.
impl<'de> Deserialize<'de> for RepodataConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RepodataConfigVisitor;

        impl<'de> serde::de::Visitor<'de> for RepodataConfigVisitor {
            type Value = RepodataConfig;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a repodata config map")
            }

            fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let mut default = RepodataChannelConfig::default();
                let mut per_channel = HashMap::new();

                while let Some(key) = access.next_key::<String>()? {
                    match key.as_str() {
                        "disable-bzip2" | "disable_bzip2" => {
                            default.disable_bzip2 = Some(access.next_value()?);
                        }
                        "disable-zstd" | "disable_zstd" => {
                            default.disable_zstd = Some(access.next_value()?);
                        }
                        "disable-sharded" | "disable_sharded" => {
                            default.disable_sharded = Some(access.next_value()?);
                        }
                        other => {
                            if let Ok(url) = Url::parse(other) {
                                per_channel.insert(url, access.next_value()?);
                            } else {
                                // Unknown / deprecated key — consume the value
                                // and drop it. `serde_ignored` (if wrapping the
                                // outer deserializer) will report this key.
                                let _: serde::de::IgnoredAny = access.next_value()?;
                            }
                        }
                    }
                }

                Ok(RepodataConfig {
                    default,
                    per_channel,
                })
            }
        }

        deserializer.deserialize_map(RepodataConfigVisitor)
    }
}

impl RepodataConfig {
    pub fn is_empty(&self) -> bool {
        self.default.is_empty() && self.per_channel.is_empty()
    }
}

impl Config for RepodataConfig {
    /// Merge the given `RepodataConfig` into the current one.
    /// The `other` configuration takes priority over the current one.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        // Note: `RepodataChannelConfig::merge` keeps `self` and only fills
        // the gaps from its argument, so the higher-priority side must be
        // the receiver.
        let mut merged = self.clone();
        merged.default = other.default.clone().merge(merged.default);
        for (url, config) in &other.per_channel {
            merged
                .per_channel
                .entry(url.clone())
                .and_modify(|existing| *existing = config.clone().merge(existing.clone()))
                .or_insert_with(|| config.clone());
        }
        Ok(merged)
    }

    fn validate(&self) -> Result<(), ValidationError> {
        // An empty repodata config is the default and must be accepted
        // (every downstream that doesn't opt in to repodata tuning sees
        // exactly this shape).
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec![
            "disable-bzip2".to_string(),
            "disable-zstd".to_string(),
            "disable-sharded".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unknown / deprecated top-level keys (e.g. legacy `disable-jlap`)
    /// must NOT cause deserialization to fail. Downstreams that wrap
    /// the deserializer in `serde_ignored` surface them as warnings.
    #[test]
    fn tolerant_deserialize_ignores_unknown_top_level_keys() {
        let toml = r#"
            disable-bzip2 = true
            disable-jlap  = true
            disable-zstd  = false
        "#;
        let config: RepodataConfig = toml::from_str(toml).expect("must accept unknown keys");
        assert_eq!(config.default.disable_bzip2, Some(true));
        assert_eq!(config.default.disable_zstd, Some(false));
        assert!(config.per_channel.is_empty());
    }

    /// Snake-case spellings for known fields are still accepted.
    #[test]
    fn tolerant_deserialize_accepts_snake_case_aliases() {
        let toml = r#"
            disable_bzip2 = true
            disable_zstd  = true
        "#;
        let config: RepodataConfig = toml::from_str(toml).expect("snake_case must work");
        assert_eq!(config.default.disable_bzip2, Some(true));
        assert_eq!(config.default.disable_zstd, Some(true));
    }

    /// URL-shaped keys still route into `per_channel`.
    #[test]
    fn tolerant_deserialize_routes_url_keys_to_per_channel() {
        let toml = r#"
            disable-bzip2 = true

            ["https://conda.anaconda.org/conda-forge"]
            disable-sharded = true
        "#;
        let config: RepodataConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.default.disable_bzip2, Some(true));
        assert_eq!(config.per_channel.len(), 1);
        let key = Url::parse("https://conda.anaconda.org/conda-forge").unwrap();
        assert_eq!(
            config.per_channel.get(&key).unwrap().disable_sharded,
            Some(true)
        );
    }

    /// Per-channel sub-tables also tolerate unknown keys (we dropped
    /// `deny_unknown_fields` from `RepodataChannelConfig` so legacy
    /// per-channel `disable-jlap = true` no longer hard-errors).
    #[test]
    fn tolerant_deserialize_accepts_unknown_per_channel_keys() {
        let toml = r#"
            ["https://example.com/foo"]
            disable-jlap   = true
            disable-bzip2  = true
        "#;
        let config: RepodataConfig = toml::from_str(toml).unwrap();
        let key = Url::parse("https://example.com/foo").unwrap();
        assert_eq!(
            config.per_channel.get(&key).unwrap().disable_bzip2,
            Some(true)
        );
    }

    /// An empty (default) `RepodataConfig` must validate successfully.
    /// Previously `validate` rejected it.
    #[test]
    fn validate_accepts_empty() {
        let config = RepodataConfig::default();
        config.validate().expect("empty repodata config is valid");
    }
}
