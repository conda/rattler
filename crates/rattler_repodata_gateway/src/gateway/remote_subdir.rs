use super::{local_subdir::LocalSubdirClient, GatewayError, SourceConfig};
use crate::fetch::{fetch_repo_data, FetchRepoDataError, FetchRepoDataOptions, Variant};
use crate::gateway::error::SubdirNotFoundError;
use crate::gateway::subdir::SubdirClient;
use crate::Reporter;
use rattler_conda_types::{Channel, PackageName, Platform, RepoDataRecord};
use reqwest_middleware::ClientWithMiddleware;
use std::{path::PathBuf, sync::Arc};

pub struct RemoteSubdirClient {
    sparse: LocalSubdirClient,
}

impl RemoteSubdirClient {
    pub async fn new(
        channel: Channel,
        platform: Platform,
        client: ClientWithMiddleware,
        cache_dir: PathBuf,
        source_config: SourceConfig,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Self, GatewayError> {
        let subdir_url = channel.platform_url(platform);

        // Fetch the repodata from the remote server
        let repodata = fetch_repo_data(
            subdir_url,
            client,
            cache_dir,
            FetchRepoDataOptions {
                cache_action: source_config.cache_action,
                variant: Variant::default(),
                jlap_enabled: source_config.jlap_enabled,
                zstd_enabled: source_config.zstd_enabled,
                bz2_enabled: source_config.bz2_enabled,
            },
            reporter,
        )
        .await
        .map_err(|e| match e {
            FetchRepoDataError::NotFound(e) => {
                GatewayError::SubdirNotFoundError(SubdirNotFoundError {
                    channel: channel.clone(),
                    subdir: platform.to_string(),
                    source: e.into(),
                })
            }
            e => GatewayError::FetchRepoDataError(e),
        })?;

        // Create a new sparse repodata client that can be used to read records from the repodata.
        let sparse = LocalSubdirClient::from_channel_subdir(
            &repodata.repo_data_json_path,
            channel.clone(),
            platform.as_str(),
        )
        .await?;

        Ok(Self { sparse })
    }
}

#[async_trait::async_trait]
impl SubdirClient for RemoteSubdirClient {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        self.sparse.fetch_package_records(name, reporter).await
    }

    fn package_names(&self) -> Vec<String> {
        self.sparse.package_names()
    }
}
