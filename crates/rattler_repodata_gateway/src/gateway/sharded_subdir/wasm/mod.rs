use std::sync::Arc;

use futures::future::OptionFuture;
use rattler_conda_types::{
    Channel, ChannelRelations, PackageName, RepodataRevisions, ShardedRepodata,
};
use rattler_networking::LazyClient;
use url::Url;

use super::add_trailing_slash;

mod index;

use crate::{
    GatewayError, Reporter,
    fetch::FetchRepoDataError,
    gateway::{
        error::SubdirNotFoundError,
        sharded_subdir::{
            decode_zst_bytes_async, is_missing_sharded_repodata_status, parse_records,
        },
        subdir::{PackageRecords, SubdirClient},
    },
    reporter::ResponseReporterExt,
    sparse::PackageFormatSelection,
    utils::js_fetch::JsFetcher,
};

pub struct ShardedSubdir {
    channel: Channel,
    client: LazyClient,
    js_fetch: Option<JsFetcher>,
    shards_base_url: Url,
    package_base_url: Url,
    sharded_repodata: ShardedRepodata,
    concurrent_requests_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    package_format_selection: Option<PackageFormatSelection>,
}

impl ShardedSubdir {
    pub async fn new(
        channel: Channel,
        subdir: String,
        client: LazyClient,
        js_fetch: Option<JsFetcher>,
        package_format_selection: Option<PackageFormatSelection>,
        concurrent_requests_semaphore: Option<Arc<tokio::sync::Semaphore>>,
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
            js_fetch.clone(),
            &index_base_url,
            concurrent_requests_semaphore.clone(),
            reporter,
        )
        .await
        .map_err(|e| match e {
            GatewayError::ReqwestError(e)
                if e.status().is_some_and(is_missing_sharded_repodata_status) =>
            {
                GatewayError::SubdirNotFoundError(Box::new(SubdirNotFoundError {
                    channel: channel.clone(),
                    subdir: subdir.clone(),
                    source: e.into(),
                }))
            }
            GatewayError::JsFetchError(e)
                if e.status().is_some_and(is_missing_sharded_repodata_status) =>
            {
                GatewayError::SubdirNotFoundError(Box::new(SubdirNotFoundError {
                    channel: channel.clone(),
                    subdir: subdir.clone(),
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
            js_fetch,
            shards_base_url: add_trailing_slash(&shards_base_url).into_owned(),
            package_base_url: add_trailing_slash(&package_base_url).into_owned(),
            sharded_repodata,
            concurrent_requests_semaphore,
            package_format_selection,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl SubdirClient for ShardedSubdir {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<&dyn Reporter>,
    ) -> Result<PackageRecords, GatewayError> {
        // Find the shard that contains the package
        let Some(shard) = self.sharded_repodata.shards.get(name.as_normalized()) else {
            return Ok(PackageRecords::default());
        };

        // Download the shard
        let shard_url = self
            .shards_base_url
            .join(&format!("{}.msgpack.zst", hex::encode(shard)))
            .expect("invalid shard url");

        let shard_bytes = {
            let _request_permit = OptionFuture::from(
                self.concurrent_requests_semaphore
                    .as_deref()
                    .map(tokio::sync::Semaphore::acquire),
            )
            .await;

            match &self.js_fetch {
                Some(fetcher) => fetcher.get(&shard_url).await?.bytes,
                None => {
                    let shard_request = self
                        .client
                        .client()
                        .get(shard_url.clone())
                        .build()
                        .expect("failed to build shard request");

                    let reporter = reporter
                        .and_then(Reporter::download_reporter)
                        .map(|r| (r, r.on_download_start(&shard_url)));
                    let shard_response = self
                        .client
                        .client()
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

                    bytes::Bytes::from(bytes)
                }
            }
        };

        let shard_bytes = decode_zst_bytes_async(shard_bytes, shard_url).await?;

        // Parse the records from the shard (includes dep extraction)
        parse_records(
            shard_bytes,
            self.channel.base_url.clone(),
            self.package_base_url.clone(),
            self.package_format_selection,
        )
        .await
    }

    fn package_names(&self) -> Vec<String> {
        self.sharded_repodata.shards.keys().cloned().collect()
    }

    fn repodata_revisions(&self) -> &RepodataRevisions {
        &self.sharded_repodata.info.repodata_revisions
    }

    fn channel_relations(&self) -> Option<&ChannelRelations> {
        self.sharded_repodata.info.channel_relations.as_ref()
    }
}
