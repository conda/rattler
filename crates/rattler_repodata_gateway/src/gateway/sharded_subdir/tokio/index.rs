use std::{path::Path, str::FromStr, sync::Arc, time::SystemTime};

use super::{REPODATA_SHARDS_FILENAME, SHARDS_CACHE_SUFFIX, ShardedRepodata};
use crate::{
    GatewayError, Reporter,
    fetch::CacheAction,
    gateway::{
        error::SubdirNotFoundError,
        sharded_subdir::{decode_zst_bytes_async, is_missing_sharded_repodata_status},
    },
    reporter::{DownloadReporter, ResponseReporterExt},
    utils::url_to_cache_filename,
};
use async_fd_lock::{LockWrite, RwLockWriteGuard};
use bytes::Bytes;
use fs_err::tokio as tokio_fs;
use futures::{TryFutureExt, future::OptionFuture};
use http::{HeaderMap, Method, Uri};
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

/// `Cache-Control` synthesized for a shard index served without one.
///
/// Azure Blob Storage sends no `Cache-Control` on the shard index, so the
/// derived [`CachePolicy`] has a zero freshness lifetime and every fetch issues
/// a conditional revalidation round-trip forever. Granting a small `max-age`
/// (60 seconds) lets repeated fetches inside the window be served straight from
/// the local cache; once it lapses the retained validators (`ETag` /
/// `Last-Modified`) still drive a cheap revalidation. Responses that carry their
/// own `Cache-Control` are used verbatim, so an origin's explicit freshness
/// policy is never weakened.
const SHARD_INDEX_SYNTHETIC_CACHE_CONTROL: &str = "max-age=60";

/// Scheme of a channel served from Azure Blob Storage.
///
/// The gateway keeps the channel URL in its `az://` form throughout; the
/// networking middleware rewrites the scheme only at send time, on its own copy
/// of the request. So the scheme observed here still identifies an Azure
/// channel.
const AZURE_CHANNEL_SCHEME: &str = "az";

/// Build a [`CachePolicy`] for a shard-index response.
///
/// Responses that already carry a `Cache-Control` header are used as-is. An
/// `az://` channel that answers without one is given a small synthetic `max-age`
/// so freshness accumulates instead of forcing a revalidation on every fetch.
/// Validators are preserved either way.
///
/// The synthesis is limited to `az://` because a missing `Cache-Control` is
/// only known to be an unconfigurable property of the origin for Azure Blob
/// Storage. For any other origin the absence is a deliberate policy that the
/// client must not override.
fn shard_index_cache_policy(request: &SimpleRequest, response: &Response) -> CachePolicy {
    if response.headers().contains_key(http::header::CACHE_CONTROL)
        || request.uri().scheme_str() != Some(AZURE_CHANNEL_SCHEME)
    {
        return CachePolicy::new(request, response);
    }

    let mut synthetic = http::Response::new(());
    *synthetic.status_mut() = response.status();
    *synthetic.headers_mut() = response.headers().clone();
    synthetic.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static(SHARD_INDEX_SYNTHETIC_CACHE_CONTROL),
    );
    CachePolicy::new(request, &synthetic)
}

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
    if cache_action != CacheAction::NoCache
        && let Ok(cache_header) = read_cached_index(&mut cache_reader).await
    {
        // Check if the cache indicates the resource was unavailable
        // (404 or 501)
        if cache_header.not_found {
            tracing::debug!(
                "cached not-available response for sharded index at {channel_base_url}"
            );
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
                    if let Ok(shard_index) = read_shard_index_from_reader(&mut cache_reader).await {
                        tracing::debug!("shard index cache hit");
                        return Ok(shard_index);
                    }
                }
                BeforeRequest::Stale {
                    request: state_request,
                    ..
                } => {
                    if cache_action == CacheAction::UseCacheOnly {
                        // Cache-only and what we have may not be used, so this
                        // subdir has no sharded index we can read. The caller
                        // falls back to `repodata.json`.
                        return Err(GatewayError::ShardedIndexNotCached(
                            channel_base_url.clone().redact(),
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

                    // Check if the resource was not found (404) or not
                    // implemented (501). Treat 501 the same as 404 so we
                    // fall back to repodata.json when a server does not
                    // support sharded repodata.
                    if is_missing_sharded_repodata_status(response.status()) {
                        tracing::debug!(
                            "sharded index unavailable ({}) at {channel_base_url}, caching this result",
                            response.status()
                        );

                        // Cache the not-available response
                        let policy = CachePolicy::new(&canonical_request, &response);
                        write_not_found_cache(cache_reader.into_inner().inner_mut(), policy)
                            .await
                            .map_err(|e| {
                                GatewayError::IoError(
                                    format!(
                                        "failed to write not-found cache for shard index to {}",
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
                        AfterResponse::NotModified(refreshed, _) => {
                            // The cached file is still valid. `after_response`
                            // returns a refreshed policy derived from the stored
                            // 200 response with the 304's headers merged, so it
                            // stays storable and retains the original (or
                            // synthesized) freshness window. Persist it so the
                            // next fetch inside the window is a local cache hit
                            // instead of yet another conditional round-trip.
                            match read_cached_body(&mut cache_reader).await {
                                Ok(body) => {
                                    tracing::debug!("shard index cache was not modified");
                                    let mut guard = cache_reader.into_inner();
                                    if let Err(e) = write_shard_index_cache(
                                        guard.inner_mut(),
                                        refreshed,
                                        Bytes::from(body.clone()),
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            "failed to persist refreshed shard index cache policy: {e}"
                                        );
                                    }
                                    if let Some((reporter, index)) = download_reporter {
                                        reporter.on_download_complete(response.url(), index);
                                    }
                                    return parse_shard_index(body).await;
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
                        AfterResponse::Modified(_, _) => {
                            // Close the old file so we can create a new one.
                            tracing::debug!("shard index cache has become stale");
                            // Synthesize freshness for a Cache-Control-less
                            // response so the re-cached index does not revalidate
                            // on every subsequent fetch.
                            let policy = shard_index_cache_policy(&canonical_request, &response);
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

    if cache_action == CacheAction::ForceCacheOnly {
        return Err(GatewayError::ShardedIndexNotCached(
            channel_base_url.clone().redact(),
        ));
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

    // Check if the resource was not found (404) or not implemented (501).
    // Treat 501 the same as 404 so we fall back to repodata.json when a
    // server does not support sharded repodata.
    if is_missing_sharded_repodata_status(response.status()) {
        tracing::debug!(
            "sharded index unavailable ({}) at {channel_base_url}, caching this result",
            response.status()
        );

        // Cache the not-available response
        let policy = CachePolicy::new(&canonical_request, &response);
        write_not_found_cache(cache_reader.into_inner().inner_mut(), policy)
            .await
            .map_err(|e| {
                GatewayError::IoError(
                    format!(
                        "failed to write not-found cache for shard index to {}",
                        cache_path.display()
                    ),
                    e,
                )
            })?;

        // Return SubdirNotFoundError to trigger fallback
        return Err(create_subdir_not_found_error(channel_base_url));
    }

    let policy = shard_index_cache_policy(&canonical_request, &response);
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

/// Writes a not-available marker (404 or 501) to the cache file.
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

/// Read the remaining bytes (the cached shard-index body) from a reader.
async fn read_cached_body<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<Vec<u8>, GatewayError> {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| GatewayError::IoError("failed to read shard index buffer".to_string(), e))?;
    Ok(bytes)
}

/// Deserialize a shard index from raw `msgpack` bytes.
async fn parse_shard_index(bytes: Vec<u8>) -> Result<ShardedRepodata, GatewayError> {
    run_blocking_task(move || {
        rmp_serde::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
            .map_err(|e| GatewayError::IoError("failed to parse shard index".to_string(), e))
    })
    .await
}

/// Read the shard index from a reader and deserialize it.
pub async fn read_shard_index_from_reader<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<ShardedRepodata, GatewayError> {
    let bytes = read_cached_body(reader).await?;
    parse_shard_index(bytes).await
}

/// Cache information stored at the start of the cache file.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheHeader {
    pub policy: CachePolicy,
    /// Indicates whether the resource was reported as unavailable (404 Not
    /// Found or 501 Not Implemented) by the remote.
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use http::{StatusCode, header};
    use http_cache_semantics::{AfterResponse, BeforeRequest};
    use url::Url;

    use super::{SimpleRequest, shard_index_cache_policy};

    /// Builds a bodyless `reqwest::Response`, mirroring how an origin (e.g.
    /// Azure Blob Storage) answers for a shard index.
    fn response(status: StatusCode, etag: &str, cache_control: Option<&str>) -> reqwest::Response {
        let mut builder = http::Response::builder()
            .status(status)
            .header(header::ETAG, etag);
        if let Some(cache_control) = cache_control {
            builder = builder.header(header::CACHE_CONTROL, cache_control);
        }
        reqwest::Response::from(builder.body(Vec::new()).unwrap())
    }

    /// Shard index URL for a channel, in the scheme the gateway holds it in.
    fn shards_url(scheme: &str) -> Url {
        Url::parse(&format!(
            "{scheme}://example.blob.core.windows.net/channel/noarch/repodata_shards.msgpack.zst"
        ))
        .unwrap()
    }

    /// The synthesis is scoped to Azure channels: an `az://` response without a
    /// `Cache-Control` gains a freshness window, while any other origin keeps the
    /// zero-freshness policy that the absent header implies.
    #[test]
    fn synthesis_is_limited_to_azure_channels() {
        let now = SystemTime::now();
        let no_cache_control = || response(StatusCode::OK, "\"v1\"", None);

        let azure = SimpleRequest::get(&shards_url("az"));
        assert!(
            shard_index_cache_policy(&azure, &no_cache_control()).time_to_live(now)
                > Duration::ZERO,
            "an az:// shard index must be given a synthetic freshness window"
        );

        let https = SimpleRequest::get(&shards_url("https"));
        assert_eq!(
            shard_index_cache_policy(&https, &no_cache_control()).time_to_live(now),
            Duration::ZERO,
            "a non-Azure origin's missing Cache-Control must be honoured as-is"
        );
    }

    /// A 304 revalidation must persist a policy that keeps a non-zero freshness
    /// window, so a fetch within that window is a local cache hit instead of yet
    /// another conditional round-trip.
    #[test]
    fn revalidation_policy_retains_freshness() {
        let url = shards_url("az");
        let request = SimpleRequest::get(&url);
        let now = SystemTime::now();

        // The original 200 carried no `Cache-Control`, so `shard_index_cache_policy`
        // synthesizes a small `max-age` and the stored policy starts fresh.
        let original = response(StatusCode::OK, "\"v1\"", None);
        let stored = shard_index_cache_policy(&request, &original);
        assert!(matches!(
            stored.before_request(&request, now),
            BeforeRequest::Fresh(_)
        ));

        // A later revalidation is answered with a 304 that, like Azure, carries no
        // `Cache-Control`. Routing it through `after_response` yields the policy
        // that is persisted for the next fetch.
        let not_modified = response(StatusCode::NOT_MODIFIED, "\"v1\"", None);
        let refreshed = match stored.after_response(&request, &not_modified, now) {
            AfterResponse::NotModified(policy, _) => policy,
            AfterResponse::Modified(_, _) => {
                panic!("a matching ETag must revalidate as NotModified")
            }
        };

        // The refreshed policy must retain a non-zero freshness window so the next
        // fetch is served straight from the cache.
        assert!(
            refreshed.time_to_live(now) > Duration::ZERO,
            "refreshed policy lost its freshness window"
        );
        assert!(matches!(
            refreshed.before_request(&request, now),
            BeforeRequest::Fresh(_)
        ));

        // Regression guard: building a policy directly from the 304 (a status that
        // is not storable) yields zero freshness, which would force perpetual
        // revalidation.
        let from_304 = shard_index_cache_policy(&request, &not_modified);
        assert_eq!(from_304.time_to_live(now), Duration::ZERO);
    }
}
