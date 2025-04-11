use std::{path::Path, sync::Arc};

use file_url::url_to_path;
use rattler_conda_types::{Channel, Platform};

use crate::{
    fetch::FetchRepoDataError,
    gateway,
    gateway::{
        error::SubdirNotFoundError,
        local_subdir::LocalSubdirClient,
        remote_subdir, sharded_subdir,
        subdir::{Subdir, SubdirData},
        GatewayInner,
    },
    GatewayError, Reporter, SourceConfig,
};

/// Builder for creating a `Subdir` instance.
pub struct SubdirBuilder<'g> {
    channel: Channel,
    platform: Platform,
    reporter: Option<Arc<dyn Reporter>>,
    gateway: &'g GatewayInner,
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
        }
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
        } else if url.scheme() == "http"
            || url.scheme() == "https"
            || url.scheme() == "gcs"
            || url.scheme() == "oci"
            || url.scheme() == "s3"
        {
            let source_config = self.gateway.channel_config.get(&self.channel.base_url);

            // Use sharded repodata if enabled
            let subdir_data = if source_config.sharded_enabled
                || gateway::force_sharded_repodata(&url)
            {
                match self.build_sharded(source_config).await {
                    Ok(client) => Some(client),
                    Err(GatewayError::SubdirNotFoundError(_)) => {
                        tracing::info!(
                            "sharded repodata seems to be missing for {url}, falling back to repodata.json files",
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

    async fn build_generic(
        &self,
        source_config: &SourceConfig,
    ) -> Result<SubdirData, GatewayError> {
        let client = remote_subdir::RemoteSubdirClient::new(
            self.channel.clone(),
            self.platform,
            self.gateway.client.clone(),
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
            #[cfg(not(target_arch = "wasm32"))]
            self.gateway.cache.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            _source_config.cache_action,
            self.gateway.concurrent_requests_semaphore.clone(),
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
