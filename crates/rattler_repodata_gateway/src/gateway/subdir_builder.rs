use std::{path::Path, sync::Arc};

use file_url::url_to_path;
use rattler_conda_types::{Channel, Platform};
use url::Url;

use crate::{
    GatewayError, Reporter, SourceConfig,
    fetch::FetchRepoDataError,
    gateway,
    gateway::{
        GatewayInner,
        error::SubdirNotFoundError,
        local_subdir::LocalSubdirClient,
        remote_subdir, sharded_subdir,
        subdir::{Subdir, SubdirData},
    },
};

/// Returns `true` for the URL schemes of remote channels the gateway can
/// fetch repodata from.
fn is_remote_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "gcs" | "oci" | "s3")
}

/// How a [`SubdirBuilder`] chooses between sharded repodata and the full
/// `repodata.json` of a remote subdir.
///
/// Local (`file://`) channels always read their `repodata.json`, and hosts
/// that only serve sharded repodata (see
/// [`force_sharded_repodata`](gateway::force_sharded_repodata)) always read
/// shards; the preference only matters for the remaining remote channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RepodataPreference {
    /// Follow the channel's [`SourceConfig`]: read sharded repodata when
    /// [`SourceConfig::sharded_enabled`] is set and the channel offers it,
    /// otherwise read the full `repodata.json`.
    #[default]
    Configured,

    /// Read the full `repodata.json` (or one of its compressed variants) and
    /// only fall back to sharded repodata when the channel has no usable
    /// full repodata: none is served, or the gateway may only read from the
    /// cache and none is cached.
    ///
    /// Reading every package of a subdir is a single request against full
    /// repodata but one request per package against shards, so scans such
    /// as [`Gateway::who_needs`](gateway::Gateway::who_needs) use this
    /// regardless of the configured sharding preference.
    PreferFull,
}

/// Builder for creating a `Subdir` instance.
pub struct SubdirBuilder<'g> {
    channel: Channel,
    platform: Platform,
    reporter: Option<Arc<dyn Reporter>>,
    gateway: &'g GatewayInner,
    repodata_preference: RepodataPreference,
}

impl<'g> SubdirBuilder<'g> {
    pub fn new(
        gateway: &'g GatewayInner,
        channel: Channel,
        platform: Platform,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Self {
        Self {
            channel,
            platform,
            reporter,
            gateway,
            repodata_preference: RepodataPreference::default(),
        }
    }

    /// Sets how the builder chooses between sharded and full repodata for
    /// remote channels. Defaults to [`RepodataPreference::Configured`].
    pub fn with_repodata_preference(self, repodata_preference: RepodataPreference) -> Self {
        Self {
            repodata_preference,
            ..self
        }
    }

    /// Returns `true` when building the subdir of `channel` and `platform`
    /// with [`RepodataPreference::PreferFull`] can produce a different
    /// subdir than building it with [`RepodataPreference::Configured`].
    ///
    /// That is the case for remote channels whose [`SourceConfig`] enables
    /// sharded repodata, unless the host only serves shards. Local channels
    /// and channels with sharding disabled read the full `repodata.json`
    /// either way, so a caller that wants full repodata can share the
    /// configured subdir for them.
    pub fn prefer_full_repodata_differs(
        gateway: &GatewayInner,
        channel: &Channel,
        platform: Platform,
    ) -> bool {
        let url = channel.platform_url(platform);
        is_remote_scheme(url.scheme())
            && !gateway::force_sharded_repodata(&url)
            && gateway
                .channel_config
                .get(&channel.base_url)
                .sharded_enabled
    }

    pub async fn build(self) -> Result<Subdir, GatewayError> {
        let url = self.channel.platform_url(self.platform);

        let subdir_data = if url.scheme() == "file" {
            if let Some(path) = url_to_path(&url) {
                self.build_local(&path).await
            } else {
                return Err(GatewayError::UnsupportedUrl(
                    "unsupported file based url".to_string(),
                ));
            }
        } else if is_remote_scheme(url.scheme()) {
            let source_config = self.gateway.channel_config.get(&self.channel.base_url);

            // Hosts that only serve sharded repodata are read through shards
            // whatever the preference; asking them for a `repodata.json`
            // would only add a failing request before the same fallback.
            if self.repodata_preference == RepodataPreference::PreferFull
                && !gateway::force_sharded_repodata(&url)
            {
                self.build_full_then_sharded(source_config, &url).await
            } else {
                self.build_configured(source_config, &url).await
            }
        } else {
            return Err(GatewayError::UnsupportedUrl(format!(
                "'{}' is not a supported scheme",
                url.scheme()
            )));
        };

        match subdir_data {
            Ok(client) => Ok(Subdir::Found(client)),
            Err(GatewayError::SubdirNotFoundError(err)) if self.platform != Platform::NoArch => {
                // If the subdir was not found and the platform is not `noarch` we assume its
                // just empty.
                tracing::info!(
                    "subdir {} of channel {} was not found, ignoring",
                    err.subdir,
                    err.channel.canonical_name()
                );
                Ok(Subdir::NotFound)
            }
            Err(GatewayError::FetchRepoDataError(FetchRepoDataError::NotFound(err))) => {
                Err(Box::new(SubdirNotFoundError {
                    subdir: self.platform.to_string(),
                    channel: self.channel.clone(),
                    source: err.into(),
                })
                .into())
            }
            Err(err) => Err(err),
        }
    }

    /// Builds the subdir as [`RepodataPreference::Configured`] describes:
    /// sharded repodata when enabled for the channel (or forced by the host)
    /// and offered by it, `repodata.json` otherwise.
    async fn build_configured(
        &self,
        source_config: &SourceConfig,
        url: &Url,
    ) -> Result<SubdirData, GatewayError> {
        // Use sharded repodata if enabled
        let subdir_data = if source_config.sharded_enabled || gateway::force_sharded_repodata(url) {
            match self.build_sharded(source_config).await {
                Ok(client) => Some(client),
                Err(GatewayError::SubdirNotFoundError(_)) => {
                    tracing::info!(
                        "sharded repodata seems to be missing for {url}, falling back to repodata.json files",
                    );
                    None
                }
                Err(GatewayError::ShardedIndexNotCached(_)) => {
                    // Cache-only mode with no usable sharded index. The
                    // channel may still be readable from a cached
                    // `repodata.json`; if it is not, the fallback reports
                    // that itself, which is the more useful error.
                    tracing::info!(
                        "no sharded repodata index is cached for {url}, falling back to repodata.json files",
                    );
                    None
                }
                Err(err) => return Err(err),
            }
        } else {
            None
        };

        // Otherwise fall back to repodata.json files
        if let Some(subdir_data) = subdir_data {
            Ok(subdir_data)
        } else {
            self.build_generic(source_config).await
        }
    }

    /// Builds the subdir as [`RepodataPreference::PreferFull`] describes:
    /// the full `repodata.json` first, sharded repodata only when the
    /// channel has no usable full repodata.
    ///
    /// The compression variants, cache action, and cache directory of the
    /// full repodata fetch come from `source_config` and the gateway exactly
    /// as for [`Self::build_configured`]; only the order of the two
    /// attempts differs.
    async fn build_full_then_sharded(
        &self,
        source_config: &SourceConfig,
        url: &Url,
    ) -> Result<SubdirData, GatewayError> {
        let full_repodata_error = match self.build_generic(source_config).await {
            Ok(subdir_data) => return Ok(subdir_data),
            // The channel serves no `repodata.json` for this subdir, or the
            // gateway may only read from the cache and none is cached. The
            // subdir may still be readable through its shards.
            Err(
                err @ (GatewayError::SubdirNotFoundError(_)
                | GatewayError::FetchRepoDataError(FetchRepoDataError::NoCacheAvailable(_))),
            ) => err,
            Err(err) => return Err(err),
        };

        tracing::info!("full repodata is unavailable for {url}, falling back to sharded repodata",);
        match self.build_sharded(source_config).await {
            Ok(subdir_data) => Ok(subdir_data),
            // Cache-only mode with no usable sharded index either. Report
            // the full repodata error, which is what an unsharded gateway
            // reports for the same cache and the more useful of the two.
            Err(GatewayError::ShardedIndexNotCached(_)) => Err(full_repodata_error),
            Err(err) => Err(err),
        }
    }

    async fn build_generic(
        &self,
        source_config: &SourceConfig,
    ) -> Result<SubdirData, GatewayError> {
        let client = remote_subdir::RemoteSubdirClient::new(
            self.channel.clone(),
            self.platform,
            self.gateway.client.clone(),
            #[cfg(target_arch = "wasm32")]
            self.gateway.js_fetch.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            self.gateway.cache.clone(),
            source_config.clone(),
            self.reporter.clone(),
        )
        .await?;
        Ok(SubdirData::from_client(client))
    }

    async fn build_sharded(
        &self,
        _source_config: &SourceConfig,
    ) -> Result<SubdirData, GatewayError> {
        let client = sharded_subdir::ShardedSubdir::new(
            self.channel.clone(),
            self.platform.to_string(),
            self.gateway.client.clone(),
            #[cfg(target_arch = "wasm32")]
            self.gateway.js_fetch.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            self.gateway.cache.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            sharded_subdir::ShardCachePolicy {
                action: _source_config.cache_action,
                missing_shards_are_empty: _source_config.missing_shards_are_empty,
            },
            self.gateway.concurrent_requests_semaphore.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            self.gateway.io_concurrency_semaphore.clone(),
            self.reporter.as_deref(),
        )
        .await?;

        Ok(SubdirData::from_client(client))
    }

    async fn build_local(&self, path: &Path) -> Result<SubdirData, GatewayError> {
        let channel = self.channel.clone();
        let platform = self.platform;
        let path = path.join("repodata.json");
        let build_client =
            move || LocalSubdirClient::from_file(&path, channel.clone(), platform.as_str());

        #[cfg(target_arch = "wasm32")]
        let client = build_client()?;
        #[cfg(not(target_arch = "wasm32"))]
        let client = simple_spawn_blocking::tokio::run_blocking_task(build_client).await?;

        Ok(SubdirData::from_client(client))
    }
}
