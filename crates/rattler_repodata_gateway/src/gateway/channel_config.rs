use crate::fetch::CacheAction;
use rattler_conda_types::Channel;
use std::collections::HashMap;

/// Describes additional properties that influence how the gateway fetches repodata for a specific
/// channel.
#[derive(Debug, Clone)]
pub struct SourceConfig {
    /// When enabled repodata can be fetched incrementally using JLAP (defaults to true)
    pub jlap_enabled: bool,

    /// When enabled, the zstd variant will be used if available (defaults to true)
    pub zstd_enabled: bool,

    /// When enabled, the bz2 variant will be used if available (defaults to true)
    pub bz2_enabled: bool,

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
            cache_action: CacheAction::default(),
        }
    }
}

/// Describes additional information for fetching channels.
#[derive(Debug, Default)]
pub struct ChannelConfig {
    /// The default source configuration. If a channel does not have a specific source configuration
    /// this configuration will be used.
    pub default: SourceConfig,

    /// Describes per channel properties that influence how the gateway fetches repodata.
    pub per_channel: HashMap<Channel, SourceConfig>,
}

impl ChannelConfig {
    /// Returns the source configuration for the given channel. If the channel does not have a
    /// specific source configuration the default source configuration will be returned.
    pub fn get(&self, channel: &Channel) -> &SourceConfig {
        self.per_channel.get(channel).unwrap_or(&self.default)
    }
}
