use crate::fetch::{FetchRepoDataError, RepoDataNotFoundError};
use crate::gateway::subdir::SubdirClient;
use crate::GatewayError;
use rattler_conda_types::{
    compute_package_url, Channel, PackageName, PackageRecord, RepoDataRecord,
};
use reqwest::{Response, StatusCode, Version};
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinError;
use url::Url;

pub struct ShardedSubdir {
    channel: Channel,
    client: ClientWithMiddleware,
    shard_base_url: Url,
    sharded_repodata: ShardedRepodata,
}

impl ShardedSubdir {
    pub async fn new(
        channel: Channel,
        subdir: String,
        client: ClientWithMiddleware,
    ) -> Result<Self, GatewayError> {
        // TODO: our sharded index only serves conda-forge so we simply override it.
        let channel =
            Channel::from_url(Url::parse("https://conda.anaconda.org/conda-forge").unwrap());

        let shard_base_url = Url::parse(&format!("https://fast.prefiks.dev/{subdir}/")).unwrap();

        // Fetch the sharded repodata from the remote server
        let repodata_shards_url = shard_base_url
            .join("repodata_shards.json")
            .expect("invalid shard base url");
        let response = client
            .get(repodata_shards_url.clone())
            .send()
            .await
            .map_err(FetchRepoDataError::from)?;

        // Check if the response was succesfull.
        if response.status() == StatusCode::NOT_FOUND {
            return Err(GatewayError::FetchRepoDataError(
                FetchRepoDataError::NotFound(RepoDataNotFoundError::from(
                    response.error_for_status().unwrap_err(),
                )),
            ));
        };

        let response = response
            .error_for_status()
            .map_err(FetchRepoDataError::from)?;

        // Parse the sharded repodata from the response
        let sharded_repodata: ShardedRepodata =
            response.json().await.map_err(FetchRepoDataError::from)?;

        Ok(Self {
            channel,
            client,
            sharded_repodata,
            shard_base_url,
        })
    }
}

#[async_trait::async_trait]
impl SubdirClient for ShardedSubdir {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        // Find the shard that contains the package
        let Some(shard) = self.sharded_repodata.shards.get(name.as_normalized()) else {
            return Ok(vec![].into());
        };

        // Download the shard
        let shard_url = self
            .shard_base_url
            .join(&format!("shards/{}.json.zst", shard.sha256))
            .expect("invalid shard url");

        let shard_response = self
            .client
            .get(shard_url.clone())
            .send()
            .await
            .and_then(|r| r.error_for_status().map_err(Into::into))
            .map_err(FetchRepoDataError::from)?;

        let shard_bytes = shard_response
            .bytes()
            .await
            .map_err(FetchRepoDataError::from)?;

        let base_url = self.sharded_repodata.info.base_url.clone();
        let channel_name = self.channel.canonical_name();
        match tokio::task::spawn_blocking(move || {
            // Decompress the shard and read the data as package records.
            let packages = zstd::decode_all(shard_bytes.as_ref())
                .and_then(|shard| {
                    serde_json::from_slice::<Shard>(&shard).map_err(std::io::Error::from)
                })
                .map(|shard| shard.packages)?;

            // Convert to repodata records
            let repodata_records: Vec<_> = packages
                .into_iter()
                .map(|(file_name, package_record)| RepoDataRecord {
                    url: base_url.join(&file_name).unwrap(),
                    channel: channel_name.clone(),
                    package_record,
                    file_name,
                })
                .collect();

            Ok(repodata_records)
        })
        .await
        .map_err(JoinError::try_into_panic)
        {
            Ok(Ok(records)) => Ok(records.into()),
            Ok(Err(err)) => Err(GatewayError::IoError(
                "failed to extract repodata records from sparse repodata".to_string(),
                err,
            )),
            Err(Ok(panic)) => std::panic::resume_unwind(panic),
            Err(Err(_)) => Err(GatewayError::IoError(
                "loading of the records was cancelled".to_string(),
                std::io::ErrorKind::Interrupted.into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardedRepodata {
    pub info: ShardedSubdirInfo,
    /// The individual shards indexed by package name.
    pub shards: HashMap<String, ShardRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardRef {
    // The sha256 hash of the shard
    pub sha256: String,

    // The size of the shard.
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shard {
    pub packages: HashMap<String, PackageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardedSubdirInfo {
    /// The name of the subdirectory
    pub subdir: String,

    /// The base url of the subdirectory. This is the location where the actual
    /// packages are stored.
    pub base_url: Url,
}
