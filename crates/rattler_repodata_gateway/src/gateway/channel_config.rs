use std::collections::HashMap;

use rattler_conda_types::ChannelUrl;
use url::Url;

use crate::fetch::CacheAction;

/// Describes additional properties that influence how the gateway fetches
/// repodata for a specific channel.
#[derive(Debug, Clone)]
pub struct SourceConfig {
    /// When enabled repodata can be fetched incrementally using JLAP (defaults
    /// to true)
    pub jlap_enabled: bool,

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
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            jlap_enabled: true,
            zstd_enabled: true,
            bz2_enabled: true,
            sharded_enabled: false,
            cache_action: CacheAction::default(),
        }
    }
}

/// Describes additional information for fetching channels.
#[derive(Debug, Default)]
pub struct ChannelConfig {
    /// The default source configuration. If a channel does not have a specific
    /// source configuration this configuration will be used.
    pub default: SourceConfig,

    /// Source configuration on a per-URL basis. This URL is used as a prefix,
    /// so any channel that starts with the URL uses the configuration.
    /// The configuration with the longest matching prefix is used.
    pub per_channel: HashMap<Url, SourceConfig>,
}

impl ChannelConfig {
    /// Returns the source configuration for the given channel. Locates the
    /// source configuration that best matches the requested channel.
    pub fn get(&self, channel: &ChannelUrl) -> &SourceConfig {
        self.per_channel
            .iter()
            .filter_map(|(url, config)| {
                let key_url = url.as_str().strip_suffix('/').unwrap_or(url.as_str());
                if channel.as_str().starts_with(key_url) {
                    Some((key_url.len(), config))
                } else {
                    None
                }
            })
            .max_by_key(|(len, _)| *len)
            .map_or(&self.default, |(_, config)| config)
    }
}
