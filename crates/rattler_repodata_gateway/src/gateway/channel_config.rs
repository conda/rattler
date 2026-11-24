use std::collections::HashMap;

use rattler_conda_types::ChannelUrl;
use url::Url;

use crate::fetch::CacheAction;

/// Describes additional properties that influence how the gateway fetches
/// repodata for a specific channel.
#[derive(Debug, Clone)]
pub struct SourceConfig {
    /// When enabled, the zstd variant will be used if available (defaults to
    /// true)
    pub zstd_enabled: bool,

    /// When enabled, the bz2 variant will be used if available (defaults to
    /// true)
    pub bz2_enabled: bool,

    /// When enabled, sharded repodata will be used if available.
    pub sharded_enabled: bool,

    /// Describes fetching repodata from a channel should interact with any
    /// caches.
    pub cache_action: CacheAction,

    /// When the gateway may only read from the cache, report a package whose
    /// shard is absent as having no records instead of failing the query.
    ///
    /// This only affects sharded channels. Their cache holds exactly the
    /// packages that earlier queries happened to walk, so a query that reaches
    /// a package none of them did finds nothing, which is not the same as the
    /// channel being unusable. Callers that restrict a solve to locally
    /// available packages want that to mean "no local candidates"; callers
    /// that solve against the full cached repodata want the error, which is
    /// why this defaults to `false`.
    ///
    /// A missing shard *index* remains an error either way: without it nothing
    /// is known about the subdir at all.
    pub missing_shards_are_empty: bool,
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            zstd_enabled: true,
            bz2_enabled: true,
            sharded_enabled: true,
            cache_action: CacheAction::default(),
            missing_shards_are_empty: false,
        }
    }
}

#[cfg(feature = "rattler_config")]
impl From<rattler_config::config::repodata_config::RepodataChannelConfig> for SourceConfig {
    fn from(value: rattler_config::config::repodata_config::RepodataChannelConfig) -> Self {
        SourceConfig {
            zstd_enabled: !value.disable_zstd.unwrap_or(false),
            bz2_enabled: !value.disable_bzip2.unwrap_or(false),
            sharded_enabled: !value.disable_sharded.unwrap_or(false),
            cache_action: CacheAction::default(),
            missing_shards_are_empty: false,
        }
    }
}

/// Describes additional information for fetching channels.
#[derive(Debug, Default, Clone)]
pub struct ChannelConfig {
    /// The default source configuration. If a channel does not have a specific
    /// source configuration this configuration will be used.
    pub default: SourceConfig,

    /// Source configuration on a per-URL basis. This URL is used as a prefix,
    /// so any channel that starts with the URL uses the configuration.
    /// The configuration with the longest matching prefix is used.
    pub per_channel: HashMap<Url, SourceConfig>,
}

/// True iff `key` equals `target` or is followed by a `/` separator in
/// `target`. Both inputs are expected to be slash-trimmed.
fn is_prefix_match(key: &str, target: &str) -> bool {
    if key == target {
        return true;
    }
    let Some(rest) = target.strip_prefix(key) else {
        return false;
    };
    rest.starts_with('/')
}

impl ChannelConfig {
    /// Returns the source configuration for the given channel. Locates the
    /// source configuration that best matches the requested channel.
    pub fn get(&self, channel: &ChannelUrl) -> &SourceConfig {
        let channel_str = channel.as_str().trim_end_matches('/');
        self.per_channel
            .iter()
            .filter_map(|(url, config)| {
                let key_url = url.as_str().trim_end_matches('/');
                if is_prefix_match(key_url, channel_str) {
                    Some((key_url.len(), config))
                } else {
                    None
                }
            })
            .max_by_key(|(len, _)| *len)
            .map_or(&self.default, |(_, config)| config)
    }
}

#[cfg(feature = "rattler_config")]
impl From<&rattler_config::config::CommonConfig> for ChannelConfig {
    fn from(config: &rattler_config::config::CommonConfig) -> Self {
        let repodata_config = &config.repodata_config;
        let default = repodata_config.default.clone().into();

        let per_channel = repodata_config
            .per_channel
            .iter()
            .map(|(url, config)| {
                (
                    url.clone(),
                    config.merge(repodata_config.default.clone()).into(),
                )
            })
            .collect();

        ChannelConfig {
            default,
            per_channel,
        }
    }
}

#[cfg(feature = "rattler_config")]
impl<T> From<&rattler_config::config::ConfigBase<T>> for ChannelConfig
where
    T: rattler_config::config::Config,
{
    fn from(config: &rattler_config::config::ConfigBase<T>) -> Self {
        Self::from(&config.common)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_channel_config_prefix_matching() {
        let mut config = ChannelConfig {
            default: SourceConfig {
                zstd_enabled: true,
                sharded_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let cf_url = Url::from_str("https://conda.anaconda.org/conda-forge").unwrap();
        let cf_config = SourceConfig {
            zstd_enabled: false,
            ..Default::default()
        };
        config.per_channel.insert(cf_url, cf_config);

        let root_url = Url::from_str("https://conda.anaconda.org").unwrap();
        let root_config = SourceConfig {
            sharded_enabled: false,
            ..Default::default()
        };
        config.per_channel.insert(root_url, root_config);

        // Exact channel match
        let cf_channel =
            ChannelUrl::from(Url::from_str("https://conda.anaconda.org/conda-forge/").unwrap());
        assert!(!config.get(&cf_channel).zstd_enabled);

        // Subchannel / label match
        let cf_label = ChannelUrl::from(
            Url::from_str("https://conda.anaconda.org/conda-forge/label/dev/").unwrap(),
        );
        assert!(!config.get(&cf_label).zstd_enabled);

        // Channel with matching prefix name must NOT match conda-forge (should match root instead)
        let cf_nightly = ChannelUrl::from(
            Url::from_str("https://conda.anaconda.org/conda-forge-nightly/").unwrap(),
        );
        assert!(config.get(&cf_nightly).zstd_enabled);
        assert!(!config.get(&cf_nightly).sharded_enabled);

        // Hostname prefix collision must NOT match
        let other_domain = ChannelUrl::from(
            Url::from_str("https://conda.anaconda.org.attacker.com/conda-forge/").unwrap(),
        );
        assert!(config.get(&other_domain).zstd_enabled);
        assert!(config.get(&other_domain).sharded_enabled);
    }
}
