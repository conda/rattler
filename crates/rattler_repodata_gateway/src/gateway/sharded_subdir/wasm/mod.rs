use std::sync::Arc;

use http::StatusCode;
use rattler_conda_types::{Channel, PackageName, RepoDataRecord, ShardedRepodata};
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

use super::add_trailing_slash;

mod index;

use crate::{
    fetch::FetchRepoDataError,
    gateway::{
        error::SubdirNotFoundError,
        sharded_subdir::{decode_zst_bytes_async, parse_records},
        subdir::SubdirClient,
    },
    reporter::ResponseReporterExt,
    GatewayError, Reporter,
};

pub struct ShardedSubdir {
    channel: Channel,
    client: ClientWithMiddleware,
    shards_base_url: Url,
    package_base_url: Url,
    sharded_repodata: ShardedRepodata,
    concurrent_requests_semaphore: Arc<tokio::sync::Semaphore>,
}

impl ShardedSubdir {
    pub async fn new(
        channel: Channel,
        subdir: String,
        client: ClientWithMiddleware,
        concurrent_requests_semaphore: Arc<tokio::sync::Semaphore>,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Self, GatewayError> {
        // Construct the base url for the shards (e.g. `<channel>/<subdir>`).
        let index_base_url = channel
            .base_url
            .url()
            .join(&format!("{subdir}/"))
            .expect("invalid subdir url");

        // Fetch the shard index
        let sharded_repodata = index::fetch_index(
            client.clone(),
            &index_base_url,
            concurrent_requests_semaphore.clone(),
            reporter,
        )
        .await
        .map_err(|e| match e {
            GatewayError::ReqwestError(e) if e.status() == Some(StatusCode::NOT_FOUND) => {
                GatewayError::SubdirNotFoundError(Box::new(SubdirNotFoundError {
                    channel: channel.clone(),
                    subdir,
                    source: e.into(),
                }))
            }
            e => e,
        })?;

        // Convert the URLs
        let shards_base_url = Url::options()
            .base_url(Some(&index_base_url))
            .parse(&sharded_repodata.info.shards_base_url)
            .map_err(|_e| {
                GatewayError::Generic(format!(
                    "shard index contains invalid `shards_base_url`: {}",
                    &sharded_repodata.info.shards_base_url
                ))
            })?;
        let package_base_url = Url::options()
            .base_url(Some(&index_base_url))
            .parse(&sharded_repodata.info.base_url)
            .map_err(|_e| {
                GatewayError::Generic(format!(
                    "shard index contains invalid `base_url`: {}",
                    &sharded_repodata.info.base_url
                ))
            })?;

        Ok(Self {
            channel,
            client,
            shards_base_url: add_trailing_slash(&shards_base_url).into_owned(),
            package_base_url: add_trailing_slash(&package_base_url).into_owned(),
            sharded_repodata,
            concurrent_requests_semaphore,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl SubdirClient for ShardedSubdir {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        // Find the shard that contains the package
        let Some(shard) = self.sharded_repodata.shards.get(name.as_normalized()) else {
            return Ok(vec![].into());
        };

        // Download the shard
        let shard_url = self
            .shards_base_url
            .join(&format!("{shard:x}.msgpack.zst"))
            .expect("invalid shard url");

        let shard_request = self
            .client
            .get(shard_url.clone())
            .build()
            .expect("failed to build shard request");

        let shard_bytes = {
            let _permit = self.concurrent_requests_semaphore.acquire();
            let reporter = reporter.map(|r| (r, r.on_download_start(&shard_url)));
            let shard_response = self
                .client
                .execute(shard_request)
                .await
                .and_then(|r| r.error_for_status().map_err(Into::into))
                .map_err(FetchRepoDataError::from)?;

            let bytes = shard_response
                .bytes_with_progress(reporter)
                .await
                .map_err(FetchRepoDataError::from)?;

            if let Some((reporter, index)) = reporter {
                reporter.on_download_complete(&shard_url, index);
            }

            bytes
        };

        let shard_bytes = decode_zst_bytes_async(shard_bytes).await?;

        // Create a future to parse the records from the shard
        let records = parse_records(
            shard_bytes,
            self.channel.base_url.clone(),
            self.package_base_url.clone(),
        )
        .await?;

        Ok(records.into())
    }

    fn package_names(&self) -> Vec<String> {
        self.sharded_repodata.shards.keys().cloned().collect()
    }
}
