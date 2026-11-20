//! Shared construction of the repodata [`Gateway`] used by every command that
//! queries channels.

use std::{collections::HashMap, str::FromStr};

use miette::{Context, IntoDiagnostic};
use rattler::{default_cache_dir, package_cache::PackageCache};
use rattler_conda_types::{Channel, ChannelOrMultiChannel, ChannelSpecifier, NamedChannelOrUrl};
use rattler_config::{ConfigBase, NoExtension};
use rattler_repodata_gateway::{ChannelConfig, Gateway, SourceConfig};
use reqwest_middleware::ClientWithMiddleware;

use super::client::repodata_cache_action;

/// Loads the rattler configuration from the default locations.
pub fn load_config() -> miette::Result<ConfigBase<NoExtension>> {
    ConfigBase::<NoExtension>::load_from_default_locations("rattler")
        .into_diagnostic()
        .context("failed to load configuration")
}

/// Resolves a channel given on the command line: a name of one of the
/// `multichannels` of the configuration or an inline `NAME=CHANNEL,...`
/// resolves to a multichannel, anything else to a single channel.
///
/// A single channel is parsed with [`Channel::from_str`] so that platform
/// selectors such as `conda-forge[linux-64]` keep working.
pub fn resolve_cli_channel(
    channel: &str,
    config: &ConfigBase<NoExtension>,
    channel_config: &rattler_conda_types::ChannelConfig,
) -> miette::Result<ChannelOrMultiChannel> {
    match ChannelSpecifier::from_str(channel).into_diagnostic()? {
        ChannelSpecifier::Channel(NamedChannelOrUrl::Name(name))
            if !config.multichannels.contains_key(&name) =>
        {
            Channel::from_str(channel, channel_config)
                .map(ChannelOrMultiChannel::Channel)
                .into_diagnostic()
        }
        ChannelSpecifier::Channel(NamedChannelOrUrl::Name(name)) => NamedChannelOrUrl::Name(name)
            .into_channel_or_multi_channel(&config.multichannels, channel_config)
            .into_diagnostic(),
        ChannelSpecifier::Channel(_) => Channel::from_str(channel, channel_config)
            .map(ChannelOrMultiChannel::Channel)
            .into_diagnostic(),
        spec @ ChannelSpecifier::MultiChannel { .. } => spec
            .resolve(&config.multichannels, channel_config)
            .into_diagnostic(),
    }
}

/// Resolves the channels given on the command line, otherwise the
/// `default-channels` from the configuration, otherwise conda-forge. A
/// channel name that refers to one of the `multichannels` of the
/// configuration resolves to that multichannel.
pub fn resolve_channels_or_multi_channels(
    cli_channels: Option<&[String]>,
    config: &ConfigBase<NoExtension>,
    channel_config: &rattler_conda_types::ChannelConfig,
) -> miette::Result<Vec<ChannelOrMultiChannel>> {
    match cli_channels {
        Some(channels) => channels
            .iter()
            .map(|channel| resolve_cli_channel(channel, config, channel_config))
            .collect(),
        None => config
            .default_channels
            .clone()
            .unwrap_or_else(|| vec![NamedChannelOrUrl::Name("conda-forge".to_string())])
            .into_iter()
            .map(|channel| {
                channel
                    .into_channel_or_multi_channel(&config.multichannels, channel_config)
                    .into_diagnostic()
            })
            .collect(),
    }
}

/// Like [`resolve_channels_or_multi_channels`], but with every multichannel
/// replaced by its channels. For commands that only query channels, where
/// the priority tiers of a multichannel do not matter.
pub fn resolve_channels(
    cli_channels: Option<&[String]>,
    config: &ConfigBase<NoExtension>,
    channel_config: &rattler_conda_types::ChannelConfig,
) -> miette::Result<Vec<Channel>> {
    Ok(
        resolve_channels_or_multi_channels(cli_channels, config, channel_config)?
            .iter()
            .flat_map(|channel| channel.channels().iter().cloned())
            .collect(),
    )
}

/// Builds the repodata gateway with the settings shared by every command:
/// the rattler cache directory (honoring `RATTLER_CACHE_DIR`) for both
/// repodata and packages, the offline cache behavior, and the download
/// concurrency from the configuration file. Only whether sharded repodata
/// is used differs per command.
pub fn build_gateway(
    client: ClientWithMiddleware,
    config: &ConfigBase<NoExtension>,
    offline: bool,
    sharded_enabled: bool,
) -> miette::Result<Gateway> {
    let cache_dir = default_cache_dir()
        .map_err(|e| miette::miette!("could not determine default cache directory: {e}"))?;
    rattler_cache::ensure_cache_dir(&cache_dir)
        .map_err(|e| miette::miette!("could not create cache directory: {e}"))?;

    Ok(Gateway::builder()
        .with_cache_dir(cache_dir.join(rattler_cache::REPODATA_CACHE_DIR))
        .with_package_cache(PackageCache::new(
            cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR),
        ))
        .with_client(client)
        .with_max_concurrent_requests(config.concurrency.downloads)
        .with_channel_config(ChannelConfig {
            default: SourceConfig {
                sharded_enabled,
                cache_action: repodata_cache_action(offline),
                ..SourceConfig::default()
            },
            per_channel: HashMap::new(),
        })
        .finish())
}
