use std::sync::Arc;

use coalesced_map::{CoalescedGetError, CoalescedMap};
use futures::future::OptionFuture;
use rattler_conda_types::{
    Channel, ChannelRelations, PackageName, RepodataRevisions, Shard, ShardedRepodata,
};
use rattler_digest::Sha256Hash;
use rattler_networking::LazyClient;
use url::Url;

use super::{add_trailing_slash, get_records, load_shard};

mod index;

use crate::{
    GatewayError, Reporter,
    fetch::FetchRepoDataError,
    gateway::{
        error::SubdirNotFoundError,
        sharded_subdir::{
            PackageFormatSelection, decode_zst_bytes_async, is_missing_sharded_repodata_status,
        },
        subdir::{PackageRecords, SubdirClient},
    },
    reporter::ResponseReporterExt,
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
    shard_cache: CoalescedMap<Sha256Hash, Arc<Shard>>,
}

impl ShardedSubdir {
    pub async fn new(
        channel: Channel,
        subdir: String,
        client: LazyClient,
        js_fetch: Option<JsFetcher>,
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
            shard_cache: CoalescedMap::new(),
        })
    }
}

impl ShardedSubdir {
    async fn get_or_fetch_shard(
        &self,
        shard_hash: Sha256Hash,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<Shard>, GatewayError> {
        self.shard_cache
            .get_or_try_init(shard_hash, || {
                self.fetch_and_parse_shard(shard_hash, reporter)
            })
            .await
            .map_err(|e| match e {
                CoalescedGetError::Init(gateway_err) => gateway_err,
                CoalescedGetError::CoalescedRequestFailed => GatewayError::IoError(
                    "a coalesced request failed".to_string(),
                    std::io::ErrorKind::Other.into(),
                ),
            })
    }

    async fn fetch_and_parse_shard(
        &self,
        shard_hash: Sha256Hash,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<Shard>, GatewayError> {
        // Download the shard
        let shard_url = self
            .shards_base_url
            .join(&format!("{}.msgpack.zst", hex::encode(shard_hash)))
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
        let shard = load_shard(shard_bytes)?;
        Ok(Arc::new(shard))
    }
}

#[async_trait::async_trait(?Send)]
impl SubdirClient for ShardedSubdir {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<&dyn Reporter>,
        package_format_selection: PackageFormatSelection,
    ) -> Result<PackageRecords, GatewayError> {
        // Find the shard that contains the package
        let Some(&shard_hash) = self.sharded_repodata.shards.get(name.as_normalized()) else {
            return Ok(PackageRecords::default());
        };

        let shard = self.get_or_fetch_shard(shard_hash, reporter).await?;

        Ok(get_records(
            (*shard).clone(),
            &self.channel.base_url,
            &self.package_base_url,
            package_format_selection,
        ))
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
