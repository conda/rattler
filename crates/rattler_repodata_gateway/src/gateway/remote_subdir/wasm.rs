use std::sync::Arc;

use rattler_conda_types::{Channel, Platform};
use reqwest_middleware::ClientWithMiddleware;

use crate::{
    fetch::{
        no_cache::{fetch_repo_data, FetchRepoDataOptions},
        FetchRepoDataError,
    },
    gateway::{
        error::SubdirNotFoundError, local_subdir::LocalSubdirClient, GatewayError, SourceConfig,
    },
    Reporter,
};

pub struct RemoteSubdirClient {
    pub(super) sparse: LocalSubdirClient,
}

impl RemoteSubdirClient {
    pub async fn new(
        channel: Channel,
        platform: Platform,
        client: ClientWithMiddleware,
        source_config: SourceConfig,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Self, GatewayError> {
        let subdir_url = channel.platform_url(platform);

        // Fetch the repodata from the remote server
        let repodata_bytes = fetch_repo_data(
            subdir_url,
            client,
            FetchRepoDataOptions {
                zstd_enabled: source_config.zstd_enabled,
                bz2_enabled: source_config.bz2_enabled,
                ..FetchRepoDataOptions::default()
            },
            reporter,
        )
        .await
        .map_err(|e| match e {
            FetchRepoDataError::NotFound(e) => {
                GatewayError::SubdirNotFoundError(Box::new(SubdirNotFoundError {
                    channel: channel.clone(),
                    subdir: platform.to_string(),
                    source: e.into(),
                }))
            }
            e => GatewayError::FetchRepoDataError(e),
        })?;

        // Create a new sparse repodata client that can be used to read records from the
        // repodata.
        let sparse =
            LocalSubdirClient::from_bytes(repodata_bytes, channel.clone(), platform.as_str())?;

        Ok(Self { sparse })
    }
}
