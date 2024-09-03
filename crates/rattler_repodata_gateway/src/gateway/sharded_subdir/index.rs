use std::{path::Path, str::FromStr, sync::Arc, time::SystemTime};

use async_fd_lock::{LockWrite, RwLockWriteGuard};
use bytes::Bytes;
use futures::TryFutureExt;
use http::{HeaderMap, Method, Uri};
use http_cache_semantics::{AfterResponse, BeforeRequest, CachePolicy, RequestLike};
use reqwest::Response;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use simple_spawn_blocking::tokio::run_blocking_task;
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, BufWriter},
};
use url::Url;

use super::{token::TokenClient, ShardedRepodata};
use crate::{reporter::ResponseReporterExt, utils::url_to_cache_filename, GatewayError, Reporter};

/// Magic number that identifies the cache file format.
const MAGIC_NUMBER: &[u8] = b"SHARD-CACHE-V1";

const REPODATA_SHARDS_FILENAME: &str = "repodata_shards.msgpack.zst";

// Fetches the shard index from the url or read it from the cache.
pub async fn fetch_index(
    client: ClientWithMiddleware,
    channel_base_url: &Url,
    token_client: &TokenClient,
    cache_dir: &Path,
    concurrent_requests_semaphore: Arc<tokio::sync::Semaphore>,
    reporter: Option<&dyn Reporter>,
) -> Result<ShardedRepodata, GatewayError> {
    async fn from_response(
        mut cache_file: RwLockWriteGuard<File>,
        cache_path: &Path,
        policy: CachePolicy,
        response: Response,
        reporter: Option<(&dyn Reporter, usize)>,
    ) -> Result<ShardedRepodata, GatewayError> {
        // Read the bytes of the response
        let response_url = response.url().clone();
        let bytes = response.bytes_with_progress(reporter).await?;

        if let Some((reporter, index)) = reporter {
            reporter.on_download_complete(&response_url, index);
        }

        // Decompress the bytes
        let decoded_bytes = Bytes::from(super::decode_zst_bytes_async(bytes).await?);

        // Write the cache to disk if the policy allows it.
        let cache_fut =
            write_shard_index_cache(cache_file.inner_mut(), policy, decoded_bytes.clone())
                .map_ok(Some)
                .map_err(|e| {
                    GatewayError::IoError(
                        format!(
                            "failed to create temporary file to cache shard index to {}",
                            cache_path.display()
                        ),
                        e,
                    )
                });

        // Parse the bytes
        let parse_fut = run_blocking_task(move || {
            rmp_serde::from_slice(&decoded_bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
                .map_err(|e| {
                    GatewayError::IoError(
                        format!("failed to parse shard index from {response_url}"),
                        e,
                    )
                })
        });

        // Parse and write the file to disk concurrently
        let (_, sharded_index) = tokio::try_join!(cache_fut, parse_fut)?;

        Ok(sharded_index)
    }

    // Fetch the sharded repodata from the remote server
    let canonical_shards_url = channel_base_url
        .join(REPODATA_SHARDS_FILENAME)
        .expect("invalid shard base url");

    let cache_file_name = format!(
        "{}.shards-cache-v1",
        url_to_cache_filename(&canonical_shards_url)
    );
    let cache_path = cache_dir.join(cache_file_name);

    // Make sure the cache directory exists
    if let Some(parent) = cache_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|err| {
            GatewayError::IoError(format!("failed to create '{}'", parent.display()), err)
        })?;
    }

    // Open and lock the cache file
    let cache_file = tokio::fs::OpenOptions::new()
        .write(true)
        .read(true)
        .truncate(false)
        .create(true)
        .open(&cache_path)
        .await
        .map_err(|err| {
            GatewayError::IoError(format!("failed to open '{}'", cache_path.display()), err)
        })?;

    // Acquire a lock on the file.
    let cache_lock = cache_file.lock_write().await.map_err(|err| {
        GatewayError::IoError(
            format!("failed to lock '{}'", cache_path.display()),
            err.error,
        )
    })?;
    let mut cache_reader = BufReader::new(cache_lock);

    let canonical_request = SimpleRequest::get(&canonical_shards_url);

    // Try reading the cached file
    if let Ok(cache_header) = read_cached_index(&mut cache_reader).await {
        match cache_header
            .policy
            .before_request(&canonical_request, SystemTime::now())
        {
            BeforeRequest::Fresh(_) => {
                if let Ok(shard_index) = read_shard_index_from_reader(&mut cache_reader).await {
                    tracing::debug!("shard index cache hit");
                    return Ok(shard_index);
                }
            }
            BeforeRequest::Stale {
                request: state_request,
                ..
            } => {
                // Get the token from the token client
                let token = token_client.get_token(reporter).await?;

                // Determine the actual URL to use for the request
                let shards_url = token
                    .shard_base_url
                    .as_ref()
                    .unwrap_or(channel_base_url)
                    .join(REPODATA_SHARDS_FILENAME)
                    .expect("invalid shard base url");

                // Construct the actual request that we will send
                let mut request = client
                    .get(shards_url.clone())
                    .headers(state_request.headers().clone())
                    .build()
                    .expect("failed to build request for shard index");
                token.add_to_headers(request.headers_mut());

                // Acquire a permit to do a request
                let _request_permit = concurrent_requests_semaphore.acquire().await;

                // Send the request
                let download_reporter = reporter.map(|r| (r, r.on_download_start(&shards_url)));
                let response = client.execute(request).await?;

                match cache_header.policy.after_response(
                    &state_request,
                    &response,
                    SystemTime::now(),
                ) {
                    AfterResponse::NotModified(_policy, _) => {
                        // The cached file is still valid
                        match read_shard_index_from_reader(&mut cache_reader).await {
                            Ok(shard_index) => {
                                tracing::debug!("shard index cache was not modified");
                                // If reading the file failed for some reason we'll just fetch it
                                // again.
                                return Ok(shard_index);
                            }
                            Err(e) => {
                                tracing::warn!("the cached shard index has been corrupted: {e}");
                                if let Some((reporter, index)) = download_reporter {
                                    reporter.on_download_complete(response.url(), index);
                                }
                            }
                        }
                    }
                    AfterResponse::Modified(policy, _) => {
                        // Close the old file so we can create a new one.
                        tracing::debug!("shard index cache has become stale");
                        return from_response(
                            cache_reader.into_inner(),
                            &cache_path,
                            policy,
                            response,
                            download_reporter,
                        )
                        .await;
                    }
                }
            }
        }
    };

    tracing::debug!("fetching fresh shard index");

    // Get the token from the token client
    let token = token_client.get_token(reporter).await?;

    // Determine the actual URL to use for the request
    let shards_url = token
        .shard_base_url
        .as_ref()
        .unwrap_or(channel_base_url)
        .join(REPODATA_SHARDS_FILENAME)
        .expect("invalid shard base url");

    // Construct the actual request that we will send
    let mut request = client
        .get(shards_url.clone())
        .build()
        .expect("failed to build request for shard index");
    token.add_to_headers(request.headers_mut());

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

    let policy = CachePolicy::new(&canonical_request, &response);
    from_response(
        cache_reader.into_inner(),
        &cache_path,
        policy,
        response,
        reporter,
    )
    .await
}

/// Writes the shard index cache to disk.
async fn write_shard_index_cache(
    cache_file: &mut File,
    policy: CachePolicy,
    decoded_bytes: Bytes,
) -> std::io::Result<()> {
    let cache_header =
        rmp_serde::encode::to_vec(&CacheHeader { policy }).expect("failed to encode cache header");

    // Move to the start of the file
    cache_file.rewind().await?;

    // Write the cache to disk
    let mut writer = BufWriter::new(cache_file);
    writer.write_all(MAGIC_NUMBER).await?;
    writer
        .write_all(&(cache_header.len() as u32).to_le_bytes())
        .await?;
    writer.write_all(&cache_header).await?;
    writer.write_all(decoded_bytes.as_ref()).await?;
    writer.flush().await?;

    // Truncate the file to the correct size
    let cache_file = writer.into_inner();
    let len = cache_file.stream_position().await?;
    cache_file.set_len(len).await?;

    Ok(())
}

/// Read the shard index from a reader and deserialize it.
async fn read_shard_index_from_reader<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<ShardedRepodata, GatewayError> {
    // Read the file to memory
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| GatewayError::IoError("failed to read shard index buffer".to_string(), e))?;

    // Deserialize the bytes
    run_blocking_task(move || {
        rmp_serde::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
            .map_err(|e| GatewayError::IoError("failed to parse shard index".to_string(), e))
    })
    .await
}

/// Cache information stored at the start of the cache file.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheHeader {
    pub policy: CachePolicy,
}

/// Try reading the cache file from disk.
async fn read_cached_index<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> std::io::Result<CacheHeader> {
    // Read the magic from the file
    let mut magic_number = [0; MAGIC_NUMBER.len()];
    reader.read_exact(&mut magic_number).await?;
    if magic_number != MAGIC_NUMBER {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid magic number",
        ));
    }

    // Read the length of the header
    let header_length = reader.read_u32_le().await? as usize;

    // Read the header from the file
    let mut header_bytes = vec![0; header_length];
    reader.read_exact(&mut header_bytes).await?;

    // Deserialize the header
    let cache_header = rmp_serde::from_slice::<CacheHeader>(&header_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    Ok(cache_header)
}

/// A helper struct to make it easier to construct something that implements
/// [`RequestLike`].
struct SimpleRequest {
    uri: Uri,
    method: Method,
    headers: HeaderMap,
}

impl SimpleRequest {
    pub fn get(url: &Url) -> Self {
        Self {
            uri: Uri::from_str(url.as_str()).expect("failed to convert Url to Uri"),
            method: Method::GET,
            headers: HeaderMap::default(),
        }
    }
}

impl RequestLike for SimpleRequest {
    fn method(&self) -> &Method {
        &self.method
    }

    fn uri(&self) -> Uri {
        self.uri.clone()
    }

    fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    fn is_same_uri(&self, other: &Uri) -> bool {
        &self.uri() == other
    }
}
