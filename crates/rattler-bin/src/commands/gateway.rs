//! Shared construction of the repodata [`Gateway`] used by every command that
//! queries channels.

use std::collections::HashMap;

use miette::{Context, IntoDiagnostic};
use rattler::{default_cache_dir, package_cache::PackageCache};
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
