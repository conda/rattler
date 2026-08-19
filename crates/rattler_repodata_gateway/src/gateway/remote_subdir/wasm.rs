use std::sync::Arc;

use rattler_conda_types::{Channel, Platform};
use rattler_networking::LazyClient;

use crate::{
    Reporter,
    fetch::{
        FetchRepoDataError,
        no_cache::{FetchRepoDataOptions, fetch_repo_data, fetch_repo_data_js},
    },
    gateway::{
        GatewayError, SourceConfig, error::SubdirNotFoundError, local_subdir::LocalSubdirClient,
    },
    utils::js_fetch::JsFetcher,
};

pub struct RemoteSubdirClient {
    pub(super) sparse: LocalSubdirClient,
}

impl RemoteSubdirClient {
    pub async fn new(
        channel: Channel,
        platform: Platform,
        client: LazyClient,
        js_fetch: Option<JsFetcher>,
        source_config: SourceConfig,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Self, GatewayError> {
        let subdir_url = channel.platform_url(platform);
        let options = FetchRepoDataOptions {
            zstd_enabled: source_config.zstd_enabled,
            bz2_enabled: source_config.bz2_enabled,
            ..FetchRepoDataOptions::default()
        };

        // Fetch the repodata from the remote server
        let repodata_bytes = match js_fetch {
            Some(fetcher) => fetch_repo_data_js(subdir_url, fetcher, options).await,
            None => fetch_repo_data(subdir_url, client, options, reporter).await,
        }
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
