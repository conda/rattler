use std::sync::Arc;

use bytes::Bytes;
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

use super::ShardedRepodata;
use crate::{
    gateway::sharded_subdir::decode_zst_bytes_async, reporter::ResponseReporterExt, GatewayError,
    Reporter,
};

const REPODATA_SHARDS_FILENAME: &str = "repodata_shards.msgpack.zst";

// Fetches the shard index from the url or read it from the cache.
pub async fn fetch_index(
    client: ClientWithMiddleware,
    channel_base_url: &Url,
    concurrent_requests_semaphore: Arc<tokio::sync::Semaphore>,
    reporter: Option<&dyn Reporter>,
) -> Result<ShardedRepodata, GatewayError> {
    // Determine the actual URL to use for the request
    let shards_url = channel_base_url
        .join(REPODATA_SHARDS_FILENAME)
        .expect("invalid shard base url");

    // Construct the actual request that we will send
    let request = client
        .get(shards_url.clone())
        .build()
        .expect("failed to build request for shard index");

    // Acquire a permit to do a request
    let _request_permit = concurrent_requests_semaphore.acquire().await;

    // Do a fresh requests
    let reporter = reporter.map(|r| (r, r.on_download_start(&shards_url)));
    let response = client
        .execute(
            request
                .try_clone()
                .expect("failed to clone initial request"),
        )
        .await?;

    let response = response.error_for_status()?;

    // Read the bytes of the response
    let response_url = response.url().clone();
    let bytes = response.bytes_with_progress(reporter).await?;

    if let Some((reporter, index)) = reporter {
        reporter.on_download_complete(&response_url, index);
    }

    // Decompress the bytes
    let decoded_bytes = Bytes::from(decode_zst_bytes_async(bytes).await?);

    // Parse the bytes
    let sharded_index = rmp_serde::from_slice(&decoded_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
        .map_err(|e| {
            GatewayError::IoError(
                format!("failed to parse shard index from {response_url}"),
                e,
            )
        })?;

    Ok(sharded_index)
}
