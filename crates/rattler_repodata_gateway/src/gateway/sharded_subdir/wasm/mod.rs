use std::sync::Arc;

use futures::future::OptionFuture;
use http::StatusCode;
use rattler_conda_types::{Channel, PackageName, ShardedRepodata};
use rattler_networking::{retry_policies::default_retry_policy, LazyClient};
use retry_policies::{RetryDecision, RetryPolicy};
use url::Url;

use super::{add_trailing_slash, is_transient_error};

mod index;

use crate::{
    fetch::FetchRepoDataError,
    gateway::{
        error::SubdirNotFoundError,
        sharded_subdir::{decode_zst_bytes_async, parse_records},
        subdir::{PackageRecords, SubdirClient},
    },
    reporter::ResponseReporterExt,
    GatewayError, Reporter,
};

pub struct ShardedSubdir {
    channel: Channel,
    client: LazyClient,
    shards_base_url: Url,
    package_base_url: Url,
    sharded_repodata: ShardedRepodata,
    concurrent_requests_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// Shared backoff deadline. When a 429 is received, this is set so that
    /// all concurrent requests to the same host wait before retrying.
    backoff_until: Arc<tokio::sync::Mutex<Option<wasmtimer::tokio::Instant>>>,
}

impl ShardedSubdir {
    pub async fn new(
        channel: Channel,
        subdir: String,
        client: LazyClient,
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
            backoff_until: Arc::default(),
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
            .join(&format!("{shard:x}.msgpack.zst"))
            .expect("invalid shard url");

        let retry_policy = default_retry_policy();
        let mut retry_count = 0u32;

        let shard_bytes = loop {
            // If another request recently received a 429, wait for the shared
            // backoff deadline before sending a new request.
            {
                let deadline = *self.backoff_until.lock().await;
                if let Some(deadline) = deadline {
                    wasmtimer::tokio::sleep_until(deadline).await;
                }
            }

            let shard_request = self
                .client
                .client()
                .get(shard_url.clone())
                .build()
                .expect("failed to build shard request");

            let _request_permit = OptionFuture::from(
                self.concurrent_requests_semaphore
                    .as_deref()
                    .map(tokio::sync::Semaphore::acquire),
            )
            .await;

            let request_start = std::time::SystemTime::now();
            let reporter = reporter
                .and_then(Reporter::download_reporter)
                .map(|r| (r, r.on_download_start(&shard_url)));

            let result = async {
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

                Ok::<_, GatewayError>(bytes)
            }
            .await;

            match result {
                Ok(bytes) => break bytes,
                Err(err) if is_transient_error(&err) => {
                    match retry_policy.should_retry(request_start, retry_count) {
                        RetryDecision::Retry { execute_after } => {
                            let sleep_duration = execute_after
                                .duration_since(std::time::SystemTime::now())
                                .unwrap_or_default();

                            // Set the shared backoff deadline so other concurrent
                            // requests also wait instead of hammering the server.
                            {
                                let new_deadline =
                                    wasmtimer::tokio::Instant::now() + sleep_duration;
                                let mut backoff = self.backoff_until.lock().await;
                                if backoff.map_or(true, |d| new_deadline > d) {
                                    *backoff = Some(new_deadline);
                                }
                            }

                            tracing::warn!(
                                "transient error fetching shard {}: {}. Retry #{}, sleeping {sleep_duration:?}...",
                                shard_url,
                                err,
                                retry_count + 1,
                            );
                            wasmtimer::tokio::sleep(sleep_duration).await;
                            retry_count += 1;
                        }
                        RetryDecision::DoNotRetry => return Err(err),
                    }
                }
                Err(err) => return Err(err),
            }
        };

        let shard_bytes = decode_zst_bytes_async(shard_bytes, shard_url).await?;

        // Parse the records from the shard (includes dep extraction)
        parse_records(
            shard_bytes,
            self.channel.base_url.clone(),
            self.package_base_url.clone(),
        )
        .await
    }

    fn package_names(&self) -> Vec<String> {
        self.sharded_repodata.shards.keys().cloned().collect()
    }
}
