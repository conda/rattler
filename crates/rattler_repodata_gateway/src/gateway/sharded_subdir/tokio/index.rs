use std::{path::Path, str::FromStr, sync::Arc, time::SystemTime};

use super::{ShardedRepodata, REPODATA_SHARDS_FILENAME, SHARDS_CACHE_SUFFIX};
use crate::{
    fetch::CacheAction,
    gateway::{error::SubdirNotFoundError, sharded_subdir::decode_zst_bytes_async},
    reporter::{DownloadReporter, ResponseReporterExt},
    utils::url_to_cache_filename,
    GatewayError, Reporter,
};
use async_fd_lock::{LockWrite, RwLockWriteGuard};
use bytes::Bytes;
use fs_err::tokio as tokio_fs;
use futures::{future::OptionFuture, TryFutureExt};
use http::{HeaderMap, Method, StatusCode, Uri};
use http_cache_semantics::{AfterResponse, BeforeRequest, CachePolicy, RequestLike};
use rattler_conda_types::Channel;
use rattler_networking::LazyClient;
use rattler_redaction::Redact;
use reqwest::Response;
use serde::{Deserialize, Serialize};
use simple_spawn_blocking::tokio::run_blocking_task;
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, BufWriter},
};
use url::Url;

/// Creates a `SubdirNotFoundError` for when sharded repodata is not available.
fn create_subdir_not_found_error(channel_base_url: &Url) -> GatewayError {
    GatewayError::SubdirNotFoundError(Box::new(SubdirNotFoundError {
        channel: Channel::from_url(channel_base_url.clone()),
        subdir: channel_base_url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("unknown")
            .to_string(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "sharded repodata not found")
            .into(),
    }))
}

// Fetches the shard index from the url or read it from the cache.
pub async fn fetch_index(
    client: LazyClient,
    channel_base_url: &Url,
    cache_dir: &Path,
    cache_action: CacheAction,
    concurrent_requests_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    reporter: Option<&dyn Reporter>,
) -> Result<ShardedRepodata, GatewayError> {
    async fn from_response(
        mut cache_file: RwLockWriteGuard<File>,
        cache_path: &Path,
        policy: CachePolicy,
        response: Response,
        reporter: Option<(&dyn DownloadReporter, usize)>,
        permit: Option<tokio::sync::SemaphorePermit<'_>>,
    ) -> Result<ShardedRepodata, GatewayError> {
        let response = response.error_for_status()?;
        if !response.status().is_success() {
            let mut url = response.url().clone().redact();
            url.set_query(None);
            url.set_fragment(None);
            let status = response.status();
            let body = response.text().await.ok();
            return Err(GatewayError::ReqwestMiddlewareError(anyhow::format_err!(
                "received unexpected status code ({}) when fetching {}.\n\nBody:\n{}",
                status,
                url,
                body.as_deref().unwrap_or("<failed to get body>")
            )));
        }

        // Read the bytes of the response
        let response_url = response.url().clone();
        let bytes = response.bytes_with_progress(reporter).await?;

        if let Some((reporter, index)) = reporter {
            reporter.on_download_complete(&response_url, index);
        }

        // Decompress the bytes
        let decoded_bytes = Bytes::from(decode_zst_bytes_async(bytes, response_url.clone()).await?);

        // The response is in, so we can drop the permit
        drop(permit);

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
        "{}{}",
        url_to_cache_filename(&canonical_shards_url),
        SHARDS_CACHE_SUFFIX
    );
    let cache_path = cache_dir.join(cache_file_name);

    // Make sure the cache directory exists
    if let Some(parent) = cache_path.parent() {
        tokio_fs::create_dir_all(parent).await.map_err(|err| {
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
    if cache_action != CacheAction::NoCache {
        if let Ok(cache_header) = read_cached_index(&mut cache_reader).await {
            // Check if the cache indicates the resource was not found (404)
            if cache_header.not_found {
                tracing::debug!("cached 404 for sharded index at {channel_base_url}");
                return Err(create_subdir_not_found_error(channel_base_url));
            }

            // If we are in cache-only mode we can't fetch the index from the server
            if cache_action == CacheAction::ForceCacheOnly {
                if let Ok(shard_index) = read_shard_index_from_reader(&mut cache_reader).await {
                    tracing::debug!("using locally cached shard index for {channel_base_url}");
                    return Ok(shard_index);
                }
            } else {
                match cache_header
                    .policy
                    .before_request(&canonical_request, SystemTime::now())
                {
                    BeforeRequest::Fresh(_) => {
                        if let Ok(shard_index) =
                            read_shard_index_from_reader(&mut cache_reader).await
                        {
                            tracing::debug!("shard index cache hit");
                            return Ok(shard_index);
                        }
                    }
                    BeforeRequest::Stale {
                        request: state_request,
                        ..
                    } => {
                        if cache_action == CacheAction::UseCacheOnly {
                            return Err(GatewayError::CacheError(
                                format!("the sharded index cache for {channel_base_url} is stale and cache-only mode is enabled"),
                            ));
                        }

                        // Determine the actual URL to use for the request
                        let shards_url = channel_base_url
                            .join(REPODATA_SHARDS_FILENAME)
                            .expect("invalid shard base url");

                        // Construct the actual request that we will send
                        let request = client
                            .client()
                            .get(shards_url.clone())
                            .headers(state_request.headers().clone())
                            .build()
                            .expect("failed to build request for shard index");

                        // Acquire a permit to do a request
                        let request_permit = OptionFuture::from(
                            concurrent_requests_semaphore
                                .as_deref()
                                .map(tokio::sync::Semaphore::acquire),
                        )
                        .await
                        .transpose()
                        .expect("failed to acquire semaphore permit");

                        // Send the request
                        let download_reporter = reporter
                            .and_then(Reporter::download_reporter)
                            .map(|r| (r, r.on_download_start(&shards_url)));
                        let response = client.client().execute(request).await?;

                        // Check if the resource was not found (404)
                        if response.status() == StatusCode::NOT_FOUND {
                            tracing::debug!(
                                "sharded index not found (404) at {channel_base_url}, caching this result"
                            );

                            // Cache the 404 response
                            let policy = CachePolicy::new(&canonical_request, &response);
                            write_not_found_cache(cache_reader.into_inner().inner_mut(), policy)
                                .await
                                .map_err(|e| {
                                    GatewayError::IoError(
                                        format!(
                                            "failed to write 404 cache for shard index to {}",
                                            cache_path.display()
                                        ),
                                        e,
                                    )
                                })?;

                            if let Some((reporter, index)) = download_reporter {
                                reporter.on_download_complete(response.url(), index);
                            }

                            // Return SubdirNotFoundError to trigger fallback
                            return Err(create_subdir_not_found_error(channel_base_url));
                        }

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
                                        if let Some((reporter, index)) = download_reporter {
                                            reporter.on_download_complete(response.url(), index);
                                        }
                                        // If reading the file failed for some reason we'll just
                                        // fetch it again.
                                        return Ok(shard_index);
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "the cached shard index has been corrupted: {e}"
                                        );
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
                                    request_permit,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        }
    }

    if cache_action == CacheAction::ForceCacheOnly {
        return Err(GatewayError::CacheError(format!(
            "the sharded index cache for {channel_base_url} is not available"
        )));
    }

    tracing::debug!("fetching fresh shard index");

    // Determine the actual URL to use for the request
    let shards_url = channel_base_url
        .join(REPODATA_SHARDS_FILENAME)
        .expect("invalid shard base url");

    // Construct the actual request that we will send
    let request = client
        .client()
        .get(shards_url.clone())
        .build()
        .expect("failed to build request for shard index");

    // Acquire a permit to do a request
    let request_permit = OptionFuture::from(
        concurrent_requests_semaphore
            .as_deref()
            .map(tokio::sync::Semaphore::acquire),
    )
    .await
    .transpose()
    .expect("failed to acquire semaphore permit");

    // Do a fresh requests
    let reporter = reporter
        .and_then(Reporter::download_reporter)
        .map(|r| (r, r.on_download_start(&shards_url)));
    let response = client
        .client()
        .execute(
            request
                .try_clone()
                .expect("failed to clone initial request"),
        )
        .await?;

    // Check if the resource was not found (404)
    if response.status() == StatusCode::NOT_FOUND {
        tracing::debug!("sharded index not found (404) at {channel_base_url}, caching this result");

        // Cache the 404 response
        let policy = CachePolicy::new(&canonical_request, &response);
        write_not_found_cache(cache_reader.into_inner().inner_mut(), policy)
            .await
            .map_err(|e| {
                GatewayError::IoError(
                    format!(
                        "failed to write 404 cache for shard index to {}",
                        cache_path.display()
                    ),
                    e,
                )
            })?;

        // Return SubdirNotFoundError to trigger fallback
        return Err(create_subdir_not_found_error(channel_base_url));
    }

    let policy = CachePolicy::new(&canonical_request, &response);
    from_response(
        cache_reader.into_inner(),
        &cache_path,
        policy,
        response,
        reporter,
        request_permit,
    )
    .await
}

/// Magic number that identifies the cache file format.
const MAGIC_NUMBER: &[u8] = b"SHARD-CACHE-V1";

/// Writes cache data to disk with the given header and optional body.
async fn write_cache(
    cache_file: &mut File,
    cache_header: CacheHeader,
    body: Option<&[u8]>,
) -> std::io::Result<()> {
    let encoded_header =
        rmp_serde::encode::to_vec(&cache_header).expect("failed to encode cache header");

    // Move to the start of the file
    cache_file.rewind().await?;

    // Write the cache to disk
    let mut writer = BufWriter::new(cache_file);
    writer.write_all(MAGIC_NUMBER).await?;
    writer
        .write_all(&(encoded_header.len() as u32).to_le_bytes())
        .await?;
    writer.write_all(&encoded_header).await?;

    // Write body if present
    if let Some(body_bytes) = body {
        writer.write_all(body_bytes).await?;
    }

    writer.flush().await?;

    // Truncate the file to the correct size
    let cache_file = writer.into_inner();
    let len = cache_file.stream_position().await?;
    cache_file.set_len(len).await?;

    Ok(())
}

/// Writes the shard index cache to disk.
pub async fn write_shard_index_cache(
    cache_file: &mut File,
    policy: CachePolicy,
    decoded_bytes: Bytes,
) -> std::io::Result<()> {
    write_cache(
        cache_file,
        CacheHeader {
            policy,
            not_found: false,
        },
        Some(decoded_bytes.as_ref()),
    )
    .await
}

/// Writes a 404 (not found) marker to the cache file.
async fn write_not_found_cache(cache_file: &mut File, policy: CachePolicy) -> std::io::Result<()> {
    write_cache(
        cache_file,
        CacheHeader {
            policy,
            not_found: true,
        },
        None,
    )
    .await
}

/// Read the shard index from a reader and deserialize it.
pub async fn read_shard_index_from_reader<R: AsyncRead + Unpin>(
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
    /// Indicates whether the resource was not found (404) on the remote.
    #[serde(default)]
    pub not_found: bool,
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
pub struct SimpleRequest {
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
