use std::sync::Arc;

use bytes::Bytes;
use futures::future::OptionFuture;
use rattler_networking::LazyClient;
use url::Url;

use super::ShardedRepodata;
use crate::{
    GatewayError, Reporter, gateway::sharded_subdir::decode_zst_bytes_async,
    reporter::ResponseReporterExt, utils::js_fetch::JsFetcher,
};

const REPODATA_SHARDS_FILENAME: &str = "repodata_shards.msgpack.zst";

// Fetches the shard index from the url or read it from the cache.
pub async fn fetch_index(
    client: LazyClient,
    js_fetch: Option<JsFetcher>,
    channel_base_url: &Url,
    concurrent_requests_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    reporter: Option<&dyn Reporter>,
) -> Result<ShardedRepodata, GatewayError> {
    // Determine the actual URL to use for the request
    let shards_url = channel_base_url
        .join(REPODATA_SHARDS_FILENAME)
        .expect("invalid shard base url");

    // Acquire a permit to do a request
    let request_permit = OptionFuture::from(
        concurrent_requests_semaphore.map(tokio::sync::Semaphore::acquire_owned),
    )
    .await;

    let (bytes, response_url) = match js_fetch {
        Some(fetcher) => (fetcher.get(&shards_url).await?.bytes, shards_url.clone()),
        None => fetch_index_bytes(client, &shards_url, reporter).await?,
    };

    // Decompress the bytes
    let decoded_bytes = Bytes::from(decode_zst_bytes_async(bytes, response_url.clone()).await?);

    // Release the permit
    drop(request_permit);

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

/// Fetches the bytes of the shard index through the reqwest client.
async fn fetch_index_bytes(
    client: LazyClient,
    shards_url: &Url,
    reporter: Option<&dyn Reporter>,
) -> Result<(Bytes, Url), GatewayError> {
    // Construct the actual request that we will send
    let request = client
        .client()
        .get(shards_url.clone())
        .build()
        .expect("failed to build request for shard index");

    // Do a fresh requests
    let reporter = reporter
        .and_then(Reporter::download_reporter)
        .map(|r| (r, r.on_download_start(shards_url)));
    let response = client
        .client()
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

    Ok((Bytes::from(bytes), response_url))
}
