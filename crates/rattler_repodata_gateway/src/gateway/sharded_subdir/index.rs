use super::{token::TokenClient, ShardedRepodata};
use crate::reporter::ResponseReporterExt;
use crate::{utils::url_to_cache_filename, GatewayError, Reporter};
use bytes::Bytes;
use futures::{FutureExt, TryFutureExt};
use http::{HeaderMap, Method, Uri};
use http_cache_semantics::{AfterResponse, BeforeRequest, CachePolicy, RequestLike};
use reqwest::Response;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{io::Write, path::Path, str::FromStr, time::SystemTime};
use tempfile::NamedTempFile;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, BufReader};
use url::Url;

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
        let cache_fut = if policy.is_storable() {
            write_shard_index_cache(cache_path, policy, decoded_bytes.clone())
                .map_ok(Some)
                .map_err(|e| {
                    GatewayError::IoError(
                        format!(
                            "failed to create temporary file to cache shard index to {}",
                            cache_path.display()
                        ),
                        e,
                    )
                })
                .left_future()
        } else {
            // Otherwise delete the file
            tokio::fs::remove_file(cache_path)
                .map_ok_or_else(
                    |e| {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            Ok(None)
                        } else {
                            Err(GatewayError::IoError(
                                format!(
                                    "failed to remove cached shard index at {}",
                                    cache_path.display()
                                ),
                                e,
                            ))
                        }
                    },
                    |_| Ok(None),
                )
                .right_future()
        };

        // Parse the bytes
        let parse_fut = tokio_rayon::spawn(move || rmp_serde::from_slice(&decoded_bytes))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
            .map_err(|e| {
                GatewayError::IoError(
                    format!("failed to parse shard index from {response_url}"),
                    e,
                )
            });

        // Parse and write the file to disk concurrently
        let (temp_file, sharded_index) = tokio::try_join!(cache_fut, parse_fut)?;

        // Persist the cache if succesfully updated the cache.
        if let Some(temp_file) = temp_file {
            temp_file.persist(cache_path).map_err(|e| {
                GatewayError::IoError(
                    format!("failed to persist shard index to {}", cache_path.display()),
                    e.into(),
                )
            })?;
        }

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

    let canonical_request = SimpleRequest::get(&canonical_shards_url);

    // Try reading the cached file
    if let Ok((cache_header, file)) = read_cached_index(&cache_path).await {
        match cache_header
            .policy
            .before_request(&canonical_request, SystemTime::now())
        {
            BeforeRequest::Fresh(_) => {
                if let Ok(shard_index) = read_shard_index_from_reader(file).await {
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
                        match read_shard_index_from_reader(file).await {
                            Ok(shard_index) => {
                                tracing::debug!("shard index cache was not modified");
                                // If reading the file failed for some reason we'll just fetch it again.
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
                        drop(file);

                        tracing::debug!("shard index cache has become stale");
                        return from_response(&cache_path, policy, response, download_reporter)
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
    from_response(&cache_path, policy, response, reporter).await
}

/// Writes the shard index cache to disk.
async fn write_shard_index_cache(
    cache_path: &Path,
    policy: CachePolicy,
    decoded_bytes: Bytes,
) -> std::io::Result<NamedTempFile> {
    let cache_path = cache_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        // Write the header
        let cache_header = rmp_serde::encode::to_vec(&CacheHeader { policy })
            .expect("failed to encode cache header");
        let cache_dir = cache_path
            .parent()
            .expect("the cache path must have a parent");
        std::fs::create_dir_all(cache_dir)?;
        let mut temp_file = tempfile::Builder::new()
            .tempfile_in(cache_dir)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        temp_file.write_all(MAGIC_NUMBER)?;
        temp_file.write_all(&(cache_header.len() as u32).to_le_bytes())?;
        temp_file.write_all(&cache_header)?;
        temp_file.write_all(decoded_bytes.as_ref())?;

        Ok(temp_file)
    })
    .map_err(|e| match e.try_into_panic() {
        Ok(payload) => std::panic::resume_unwind(payload),
        Err(e) => std::io::Error::new(std::io::ErrorKind::Other, e),
    })
    .await?
}

/// Read the shard index from a reader and deserialize it.
async fn read_shard_index_from_reader(
    mut reader: BufReader<File>,
) -> std::io::Result<ShardedRepodata> {
    // Read the file to memory
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;

    // Deserialize the bytes
    tokio_rayon::spawn(move || rmp_serde::from_slice(&bytes))
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// Cache information stored at the start of the cache file.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheHeader {
    pub policy: CachePolicy,
}

/// Try reading the cache file from disk.
async fn read_cached_index(cache_path: &Path) -> std::io::Result<(CacheHeader, BufReader<File>)> {
    // Open the file for reading
    let file = File::open(cache_path).await?;
    let mut reader = BufReader::new(file);

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

    Ok((cache_header, reader))
}

/// A helper struct to make it easier to construct something that implements [`RequestLike`].
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
