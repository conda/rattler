//! file

mod cache;
pub mod jlap;

pub mod repodata;
pub mod run_exports;

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    time::SystemTime,
};

use cache_control::{Cachability, CacheControl};
use futures::{future::ready, FutureExt, TryStreamExt};
use humansize::{SizeFormatter, DECIMAL};
use rattler_digest::{compute_file_digest, Blake2b256, HashingWriter};
use rattler_networking::{
    redact_known_secrets_from_error, redact_known_secrets_from_url, DEFAULT_REDACTION_STR,
};
use reqwest::{Response, StatusCode};
use tempfile::NamedTempFile;
use tokio_util::io::StreamReader;
use tracing::instrument;
use url::Url;

pub use repodata::*;
pub use run_exports::*;

use crate::utils::{AsyncEncoding, Encoding, LockedFile};
use cache::{CacheState, Expiring};

use self::cache::CacheHeaders;

/// Type alias for function to report progress while downloading repodata
pub type ProgressFunc = Box<dyn FnMut(DownloadProgress) + Send + Sync>;

/// A struct that provides information about download progress.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// The number of bytes already downloaded
    pub bytes: u64,

    /// The total number of bytes to download. Or `None` if this is not known. This can happen
    /// if the server does not supply a `Content-Length` header.
    pub total: Option<u64>,
}

/// Data could not be found for given channel and platform
#[derive(Debug, thiserror::Error)]
pub enum DataNotFoundError {
    /// There was an error on the Http request
    #[error(transparent)]
    HttpError(reqwest::Error),

    /// There was a file system error
    #[error(transparent)]
    FileSystemError(std::io::Error),
}

impl From<reqwest::Error> for DataNotFoundError {
    fn from(err: reqwest::Error) -> Self {
        Self::HttpError(redact_known_secrets_from_error(err))
    }
}

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("failed to acquire a lock on the repodata cache")]
    FailedToAcquireLock(#[source] anyhow::Error),

    #[error(transparent)]
    HttpError(reqwest_middleware::Error),

    #[error(transparent)]
    IoError(std::io::Error),

    #[error("failed to download {0}")]
    FailedToDownload(Url, #[source] std::io::Error),

    #[error("repodata not found")]
    NotFound(#[from] DataNotFoundError),

    #[error("failed to create temporary file for data")]
    FailedToCreateTemporaryFile(#[source] std::io::Error),

    #[error("failed to persist temporary data file")]
    FailedToPersistTemporaryFile(#[from] tempfile::PersistError),

    #[error("failed to get metadata from data file")]
    FailedToGetMetadata(#[source] std::io::Error),

    #[error("failed to write cache state")]
    FailedToWriteCacheState(#[source] std::io::Error),

    #[error("there is no cache available")]
    NoCacheAvailable,

    #[error("the operation was cancelled")]
    Cancelled,
}

impl From<reqwest::Error> for FetchError {
    fn from(err: reqwest::Error) -> Self {
        Self::HttpError(redact_known_secrets_from_error(err).into())
    }
}

impl From<tokio::task::JoinError> for FetchError {
    fn from(err: tokio::task::JoinError) -> Self {
        // Rethrow any panic
        if let Ok(panic) = err.try_into_panic() {
            std::panic::resume_unwind(panic);
        }

        // Otherwise it the operation has been cancelled
        FetchError::Cancelled
    }
}

/// Defines how to use the repodata cache.
#[derive(Default, Copy, Clone, Debug, PartialEq, Eq)]
pub enum CacheAction {
    /// Use the cache if its up to date or fetch from the URL if there is no valid cached value.
    #[default]
    CacheOrFetch,

    /// Only use the cache, but error out if the cache is not up to date
    UseCacheOnly,

    /// Only use the cache, ignore whether or not it is up to date.
    ForceCacheOnly,

    /// Do not use the cache even if there is an up to date entry.
    NoCache,
}

/// Streams and decodes the response to a new temporary file in the given directory. While writing
/// to disk it also computes the BLAKE2 hash of the file.
#[instrument(skip_all)]
async fn stream_and_decode_to_file(
    url: Url,
    response: Response,
    content_encoding: Encoding,
    temp_dir: &Path,
    mut progress_func: Option<ProgressFunc>,
) -> Result<(NamedTempFile, blake2::digest::Output<Blake2b256>), FetchError> {
    // Determine the length of the response in bytes and notify the listener that a download is
    // starting. The response may be compressed. Decompression happens below.
    let content_size = response.content_length();
    if let Some(progress_func) = progress_func.as_mut() {
        progress_func(DownloadProgress {
            bytes: 0,
            total: content_size,
        });
    }

    // Determine the encoding of the response
    let transfer_encoding = Encoding::from(&response);

    // Convert the response into a byte stream
    let bytes_stream = response
        .bytes_stream()
        .map_err(|e| std::io::Error::new(ErrorKind::Other, e));

    // Listen in on the bytes as they come from the response. Progress is tracked here instead of
    // after decoding because that doesnt properly represent the number of bytes that are being
    // transferred over the network.
    let mut total_bytes = 0;
    let total_bytes_mut = &mut total_bytes;
    let bytes_stream = bytes_stream.inspect_ok(move |bytes| {
        *total_bytes_mut += bytes.len() as u64;
        if let Some(progress_func) = progress_func.as_mut() {
            progress_func(DownloadProgress {
                bytes: *total_bytes_mut,
                total: content_size,
            });
        }
    });

    // Create a new stream from the byte stream that decodes the bytes using the transfer encoding
    // on the fly.
    let decoded_byte_stream = StreamReader::new(bytes_stream).decode(transfer_encoding);

    // Create yet another stream that decodes the bytes yet again but this time using the content
    // encoding.
    let mut decoded_data_json_bytes =
        tokio::io::BufReader::new(decoded_byte_stream).decode(content_encoding);

    tracing::trace!(
        "decoding repodata (content: {:?}, transfer: {:?})",
        content_encoding,
        transfer_encoding
    );

    // Construct a temporary file
    let temp_file =
        NamedTempFile::new_in(temp_dir).map_err(FetchError::FailedToCreateTemporaryFile)?;

    // Clone the file handle and create a hashing writer so we can compute a hash while the content
    // is being written to disk.
    let file = tokio::fs::File::from_std(temp_file.as_file().try_clone().unwrap());
    let mut hashing_file_writer = HashingWriter::<_, Blake2b256>::new(file);

    // Decode, hash and write the data to the file.
    let bytes = tokio::io::copy(&mut decoded_data_json_bytes, &mut hashing_file_writer)
        .await
        .map_err(|e| {
            FetchError::FailedToDownload(
                redact_known_secrets_from_url(&url, DEFAULT_REDACTION_STR).unwrap_or(url),
                e,
            )
        })?;

    // Finalize the hash
    let (_, hash) = hashing_file_writer.finalize();

    tracing::debug!(
        "downloaded {}, decoded that into {}, BLAKE2 hash: {:x}",
        SizeFormatter::new(total_bytes, DECIMAL),
        SizeFormatter::new(bytes, DECIMAL),
        hash
    );

    Ok((temp_file, hash))
}

/// Describes the availability of certain `data`.
#[derive(Debug)]
pub struct VariantAvailability {
    has_zst: Option<Expiring<bool>>,
    has_bz2: Option<Expiring<bool>>,
    has_jlap: Option<Expiring<bool>>,
}

impl VariantAvailability {
    /// Returns true if there is a Zst variant available, regardless of when it was checked
    pub fn has_zst(&self) -> bool {
        self.has_zst.as_ref().map_or(false, |state| state.value)
    }

    /// Returns true if there is a Bz2 variant available, regardless of when it was checked
    pub fn has_bz2(&self) -> bool {
        self.has_bz2.as_ref().map_or(false, |state| state.value)
    }

    /// Returns true if there is a JLAP variant available, regardless of when it was checked
    pub fn has_jlap(&self) -> bool {
        self.has_jlap.as_ref().map_or(false, |state| state.value)
    }
}

/// Determine the availability of `data` variants (like a `.zst` or `.bz2`) by checking
/// a cache or the internet.
pub async fn check_variant_availability(
    client: &reqwest_middleware::ClientWithMiddleware,
    subdir_url: &Url,
    cache_state: Option<&CacheState>,
    filename: &str,
) -> VariantAvailability {
    // Determine from the cache which variant are available. This is currently cached for a maximum
    // of 14 days.
    let expiration_duration = chrono::Duration::days(14);
    let has_zst = cache_state
        .and_then(|state| state.has_zst.as_ref())
        .and_then(|value| value.value(expiration_duration))
        .copied();
    let has_bz2 = cache_state
        .and_then(|state| state.has_bz2.as_ref())
        .and_then(|value| value.value(expiration_duration))
        .copied();
    let has_jlap = cache_state
        .and_then(|state| state.has_jlap.as_ref())
        .and_then(|value| value.value(expiration_duration))
        .copied();

    // Create a future to possibly refresh the zst state.
    let zst_repodata_url = subdir_url.join(&format!("{filename}.zst")).unwrap();
    let bz2_repodata_url = subdir_url.join(&format!("{filename}.bz2")).unwrap();
    let jlap_repodata_url = subdir_url.join(jlap::JLAP_FILE_NAME).unwrap();

    let zst_future = match has_zst {
        Some(_) => {
            // The last cached value was valid, so we simply copy that
            ready(cache_state.and_then(|state| state.has_zst.clone())).left_future()
        }
        None => async {
            Some(Expiring {
                value: check_valid_download_target(&zst_repodata_url, client).await,
                last_checked: chrono::Utc::now(),
            })
        }
        .right_future(),
    };

    // Create a future to determine if bz2 is available. We only check this if we dont already know that
    // zst is available because if that's available we're going to use that anyway.
    let bz2_future = if has_zst == Some(true) {
        // If we already know that zst is available we simply copy the availability value from the last
        // time we checked.
        ready(cache_state.and_then(|state| state.has_zst.clone())).right_future()
    } else {
        // If the zst variant might not be available we need to check whether bz2 is available.
        async {
            match has_bz2 {
                Some(_) => {
                    // The last cached value was value so we simply copy that.
                    cache_state.and_then(|state| state.has_bz2.clone())
                }
                None => Some(Expiring {
                    value: check_valid_download_target(&bz2_repodata_url, client).await,
                    last_checked: chrono::Utc::now(),
                }),
            }
        }
        .left_future()
    };

    let jlap_future = match has_jlap {
        Some(_) => {
            // The last cached value is valid, so we simply copy that
            ready(cache_state.and_then(|state| state.has_jlap.clone())).left_future()
        }
        None => async {
            Some(Expiring {
                value: check_valid_download_target(&jlap_repodata_url, client).await,
                last_checked: chrono::Utc::now(),
            })
        }
        .right_future(),
    };

    // Await all futures so they happen concurrently. Note that a request might not actually happen if
    // the cache is still valid.
    let (has_zst, has_bz2, has_jlap) = futures::join!(zst_future, bz2_future, jlap_future);

    VariantAvailability {
        has_zst,
        has_bz2,
        has_jlap,
    }
}

/// Performs a HEAD request on the given URL to see if it is available.
async fn check_valid_download_target(
    url: &Url,
    client: &reqwest_middleware::ClientWithMiddleware,
) -> bool {
    tracing::debug!("checking availability of '{url}'");

    if url.scheme() == "file" {
        // If the url is a file url we can simply check if the file exists.
        let path = url.to_file_path().unwrap();
        let exists = tokio::fs::metadata(path).await.is_ok();
        tracing::debug!(
            "'{url}' seems to be {}",
            if exists { "available" } else { "unavailable" }
        );
        exists
    } else {
        // Otherwise, perform a HEAD request to determine whether the url seems valid.
        match client.head(url.clone()).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    tracing::debug!("'{url}' seems to be available");
                    true
                } else {
                    tracing::debug!("'{url}' seems to be unavailable");
                    false
                }
            }
            Err(e) => {
                tracing::warn!(
                    "failed to perform HEAD request on '{url}': {e}. Assuming its unavailable.."
                );
                false
            }
        }
    }
}

// Ensures that the URL contains a trailing slash. This is important for the [`Url::join`] function.
fn normalize_subdir_url(url: Url) -> Url {
    let mut path = url.path();
    path = path.trim_end_matches('/');
    let mut url = url.clone();
    url.set_path(&format!("{path}/"));
    url
}

/// dpcs
pub trait Variant: Default {
    /// Returns the file name of the repodata file to download.
    fn file_name(&self) -> &'static str;
}

#[derive(Clone)]
/// Knobs for adjusting fetch and cache
pub struct Options<V: Variant> {
    /// How to use the cache. By default it will cache and reuse downloaded data (if the
    /// server allows it).
    pub cache_action: CacheAction,

    /// Determines which variant to download. See [`Variant`] for more information.
    pub variant: V,

    /// When enabled repodata can be fetched incrementally using JLAP
    pub jlap_enabled: bool,

    /// When enabled, the zstd variant will be used if available
    pub zstd_enabled: bool,

    /// When enabled, the bz2 variant will be used if available
    pub bz2_enabled: bool,
}

/// The result of fetch
#[derive(Debug)]
pub struct CachedData {
    /// A lockfile that guards access to any of the data file or its cache.
    pub lock_file: LockedFile,

    /// The path to the uncompressed data file.
    pub path: PathBuf,

    /// The cache data.
    pub cache_state: CacheState,

    /// How the cache was used for this request.
    pub cache_result: CacheResult,
}

/// Indicates whether or not the data cache was up-to-date or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheResult {
    /// The cache was hit, the data on disk was already valid.
    CacheHit,

    /// The cache was hit, we did have to check with the server, but no data was downloaded.
    CacheHitAfterFetch,

    /// The cache was present but it was outdated.
    CacheOutdated,

    /// There was no cache available
    CacheNotPresent,
}

/// handle file:/// urls
async fn cache_from_file(
    url: Url,
    out_path: PathBuf,
    cache_state_path: PathBuf,
    lock_file: LockedFile,
) -> Result<CachedData, FetchError> {
    // copy file from subdir_url to out_path
    if let Err(e) = tokio::fs::copy(&url.to_file_path().unwrap(), &out_path).await {
        return if e.kind() == ErrorKind::NotFound {
            Err(FetchError::NotFound(DataNotFoundError::FileSystemError(e)))
        } else {
            Err(FetchError::IoError(e))
        };
    }

    // create a dummy cache state
    let new_cache_state = CacheState {
        url: url.clone(),
        cache_size: tokio::fs::metadata(&out_path)
            .await
            .map_err(FetchError::IoError)?
            .len(),
        cache_headers: CacheHeaders {
            etag: None,
            last_modified: None,
            cache_control: None,
        },
        cache_last_modified: SystemTime::now(),
        blake2_hash: None,
        blake2_hash_nominal: None,
        has_zst: None,
        has_bz2: None,
        has_jlap: None,
        jlap: None,
    };

    // write the cache state
    let new_cache_state = tokio::task::spawn_blocking(move || {
        new_cache_state
            .to_path(&cache_state_path)
            .map(|_| new_cache_state)
            .map_err(FetchError::FailedToWriteCacheState)
    })
    .await??;

    Ok(CachedData {
        lock_file,
        path: out_path.clone(),
        cache_state: new_cache_state,
        cache_result: CacheResult::CacheHit,
    })
}

/// A value returned from [`validate_cached_state`] which indicates the state of a cache.
#[derive(Debug)]
enum ValidatedCacheState {
    /// There is no cache, the cache could not be parsed, or the cache does not reference the same
    /// request. We can completely ignore any cached data.
    InvalidOrMissing,

    /// The cache does not match the data file that is on disk. This usually indicates that the
    /// data was modified without updating the cache.
    Mismatched(CacheState),

    /// The cache could be read and corresponds to the data file that is on disk but the cached
    /// data is (partially) out of date.
    OutOfDate(CacheState),

    /// The cache is up to date.
    UpToDate(CacheState),
}

/// Tries to determine if the cache state for the data for the given `subdir_url` is
/// considered to be up-to-date.
///
/// This functions reads multiple files from the `cache_path`, it is left up to the user to ensure
/// that these files stay synchronized during the execution of this function.
fn validate_cached_state(
    cache_path: &Path,
    subdir_url: &Url,
    cache_key: &str,
) -> ValidatedCacheState {
    let data_json_path = cache_path.join(format!("{cache_key}.json"));
    let cache_state_path = cache_path.join(format!("{cache_key}.info.json"));

    // Check if we have cached data file
    let json_metadata = match std::fs::metadata(&data_json_path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return ValidatedCacheState::InvalidOrMissing,
        Err(e) => {
            tracing::warn!(
                "failed to get metadata of data file '{}': {e}. Ignoring cached files...",
                data_json_path.display()
            );
            return ValidatedCacheState::InvalidOrMissing;
        }
        Ok(metadata) => metadata,
    };

    // Try to read the repodata state cache
    let cache_state = match CacheState::from_path(&cache_state_path) {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Ignore, the cache just doesnt exist
            tracing::debug!("repodata cache state is missing. Ignoring cached files...");
            return ValidatedCacheState::InvalidOrMissing;
        }
        Err(e) => {
            // An error occured while reading the cached state.
            tracing::warn!(
                "invalid repodata cache state '{}': {e}. Ignoring cached files...",
                cache_state_path.display()
            );
            return ValidatedCacheState::InvalidOrMissing;
        }
        Ok(state) => state,
    };

    // Do the URLs match?
    let cached_subdir_url = if cache_state.url.path().ends_with('/') {
        cache_state.url.clone()
    } else {
        let path = cache_state.url.path();
        let (subdir_path, _) = path.rsplit_once('/').unwrap_or(("", path));
        let mut url = cache_state.url.clone();
        url.set_path(&format!("{subdir_path}/"));
        url
    };
    if &cached_subdir_url != subdir_url {
        tracing::warn!("cache state refers to a different data url. Ignoring cached files...");
        return ValidatedCacheState::InvalidOrMissing;
    }

    // Determine last modified date of the data file.
    let cache_last_modified = match json_metadata.modified() {
        Err(_) => {
            tracing::warn!(
                "could not determine last modified date of data file. Ignoring cached files..."
            );
            return ValidatedCacheState::Mismatched(cache_state);
        }
        Ok(last_modified) => last_modified,
    };

    // Make sure that the repodata state cache refers to the repodata that exists on disk.
    //
    // Check the blake hash of the data file if we have a similar hash in the state.
    if let Some(cached_hash) = cache_state.blake2_hash.as_ref() {
        match compute_file_digest::<Blake2b256>(&data_json_path) {
            Err(e) => {
                tracing::warn!(
                    "could not compute BLAKE2 hash of data file: {e}. Ignoring cached files..."
                );
                return ValidatedCacheState::Mismatched(cache_state);
            }
            Ok(hash) => {
                if &hash != cached_hash {
                    tracing::warn!(
                        "BLAKE2 hash of data does not match cache state. Ignoring cached files..."
                    );
                    return ValidatedCacheState::InvalidOrMissing;
                }
            }
        }
    } else {
        // The state cache records the size and last modified date of the original file. If those do
        // not match, the data file has been modified.
        if json_metadata.len() != cache_state.cache_size
            || Some(cache_last_modified) != json_metadata.modified().ok()
        {
            tracing::warn!("repodata cache state mismatches the existing repodatajson file. Ignoring cached files...");
            return ValidatedCacheState::Mismatched(cache_state);
        }
    }

    // Determine the age of the cache
    let cache_age = match SystemTime::now().duration_since(cache_last_modified) {
        Ok(duration) => duration,
        Err(e) => {
            tracing::warn!("failed to determine cache age: {e}. Ignoring cached files...");
            return ValidatedCacheState::Mismatched(cache_state);
        }
    };

    // Parse the cache control header, and determine if the cache is out of date or not.
    if let Some(cache_control) = cache_state.cache_headers.cache_control.as_deref() {
        match CacheControl::from_value(cache_control) {
            None => {
                tracing::warn!(
                "could not parse cache_control from repodata cache state. Ignoring cached files..."
            );
                return ValidatedCacheState::Mismatched(cache_state);
            }
            Some(CacheControl {
                cachability: Some(Cachability::Public),
                max_age: Some(duration),
                ..
            }) => {
                if cache_age > duration {
                    tracing::debug!(
                        "Cache is {} old but can at most be {} old. Assuming out of date...",
                        humantime::format_duration(cache_age),
                        humantime::format_duration(duration),
                    );
                    return ValidatedCacheState::OutOfDate(cache_state);
                }
            }
            Some(_) => {
                tracing::debug!(
                    "Unsupported cache-control value '{}'. Assuming out of date...",
                    cache_control
                );
                return ValidatedCacheState::OutOfDate(cache_state);
            }
        }
    } else {
        tracing::warn!(
            "previous cache state does not contain cache_control header. Assuming out of date..."
        );
        return ValidatedCacheState::OutOfDate(cache_state);
    }

    // Well then! If we get here, it means the cache must be up to date!
    ValidatedCacheState::UpToDate(cache_state)
}

#[instrument(err, skip_all, fields(channel_platform_url, cache_path = %cache_path.display()))]
async fn _fetch_data<V: Variant>(
    channel_platform_url: Url,
    client: reqwest_middleware::ClientWithMiddleware,
    cache_path: PathBuf,
    options: Options<V>,
    progress: Option<ProgressFunc>,
) -> Result<CachedData, FetchError> {
    let subdir_url = normalize_subdir_url(channel_platform_url);

    // Compute the cache key from the url
    let cache_key = crate::utils::url_to_cache_filename(
        &subdir_url
            .join(options.variant.file_name())
            .expect("file name is valid"),
    );
    let data_json_path = cache_path.join(format!("{cache_key}.json"));
    let cache_state_path = cache_path.join(format!("{cache_key}.info.json"));

    // Lock all files that have to do with that cache key
    let lock_file_path = cache_path.join(format!("{}.lock", &cache_key));
    let lock_file =
        tokio::task::spawn_blocking(move || LockedFile::open_rw(lock_file_path, "repodata cache"))
            .await?
            .map_err(FetchError::FailedToAcquireLock)?;

    let cache_action = if subdir_url.scheme() == "file" {
        // If we are dealing with a local file, we can skip the cache entirely.
        return cache_from_file(
            subdir_url.join(options.variant.file_name()).unwrap(),
            data_json_path,
            cache_state_path,
            lock_file,
        )
        .await;
    } else {
        options.cache_action
    };

    // Validate the current state of the cache
    let cache_state = if cache_action == CacheAction::NoCache {
        None
    } else {
        let owned_subdir_url = subdir_url.clone();
        let owned_cache_path = cache_path.clone();
        let owned_cache_key = cache_key.clone();
        let cache_state = tokio::task::spawn_blocking(move || {
            validate_cached_state(&owned_cache_path, &owned_subdir_url, &owned_cache_key)
        })
        .await?;
        match (cache_state, options.cache_action) {
            (ValidatedCacheState::UpToDate(cache_state), _)
            | (ValidatedCacheState::OutOfDate(cache_state), CacheAction::ForceCacheOnly) => {
                // Cache is up to date or we dont care about whether or not its up to date,
                // so just immediately return what we have.
                return Ok(CachedData {
                    lock_file,
                    path: data_json_path,
                    cache_state,
                    cache_result: CacheResult::CacheHit,
                });
            }
            (ValidatedCacheState::OutOfDate(_), CacheAction::UseCacheOnly)
            | (
                ValidatedCacheState::Mismatched(_) | ValidatedCacheState::InvalidOrMissing,
                CacheAction::UseCacheOnly | CacheAction::ForceCacheOnly,
            ) => {
                // The cache is out of date but we also cant fetch new data
                // OR, The cache doesn't match the data that is on disk. This means the cache is
                // not usable.
                // OR, No cache available at all, and we cant refresh the data.
                return Err(FetchError::NoCacheAvailable);
            }
            (
                ValidatedCacheState::OutOfDate(cache_state)
                | ValidatedCacheState::Mismatched(cache_state),
                _,
            ) => {
                // The cache is out of date but we can still refresh the data
                // OR, The cache doesn't match the data that is on disk. but it might contain some other
                // interesting cached data as well...
                Some(cache_state)
            }
            (ValidatedCacheState::InvalidOrMissing, _) => {
                // No cache available but we can update it!
                None
            }
        }
    };

    // Determine the availability of variants based on the cache or by querying the remote.
    let variant_availability = check_variant_availability(
        &client,
        &subdir_url,
        cache_state.as_ref(),
        options.variant.file_name(),
    )
    .await;

    // Now that the caches have been refreshed determine whether or not we can use one of the
    // variants. We don't check the expiration here since we just refreshed it.
    let has_zst = options.zstd_enabled && variant_availability.has_zst();
    let has_bz2 = options.bz2_enabled && variant_availability.has_bz2();
    let has_jlap = options.jlap_enabled && variant_availability.has_jlap();

    // We first attempt to make a JLAP request; if it fails for any reason, we continue on with
    // a normal request.
    let jlap_state = if has_jlap && cache_state.is_some() {
        let data_state = cache_state.as_ref().unwrap();
        match crate::fetch::jlap::patch_repo_data(
            &client,
            subdir_url.clone(),
            data_state.clone(),
            &data_json_path,
        )
        .await
        {
            Ok((state, disk_hash)) => {
                tracing::info!("fetched JLAP patches successfully");
                let cache_state = CacheState {
                    blake2_hash: Some(disk_hash),
                    blake2_hash_nominal: Some(state.footer.latest),
                    has_zst: variant_availability.has_zst,
                    has_bz2: variant_availability.has_bz2,
                    has_jlap: variant_availability.has_jlap,
                    jlap: Some(state),
                    .. cache_state.expect("we must have had a cache, otherwise we wouldn't know the previous state of the cache")
                };

                let cache_state = tokio::task::spawn_blocking(move || {
                    cache_state
                        .to_path(&cache_state_path)
                        .map(|_| cache_state)
                        .map_err(FetchError::FailedToWriteCacheState)
                })
                .await??;

                return Ok(CachedData {
                    lock_file,
                    path: data_json_path,
                    cache_state,
                    cache_result: CacheResult::CacheOutdated,
                });
            }
            Err(error) => {
                tracing::warn!("Error during JLAP request: {}", error);
                None
            }
        }
    } else {
        None
    };

    // Determine which variant to download
    let data_url = if has_zst {
        subdir_url
            .join(&format!("{}.zst", options.variant.file_name()))
            .unwrap()
    } else if has_bz2 {
        subdir_url
            .join(&format!("{}.bz2", options.variant.file_name()))
            .unwrap()
    } else {
        subdir_url.join(options.variant.file_name()).unwrap()
    };

    // Construct the HTTP request
    tracing::debug!("fetching '{}'", &data_url);
    let request_builder = client.get(data_url.clone());

    let mut headers = reqwest::header::HeaderMap::default();

    // We can handle g-zip encoding which is often used. We could also set this option on the
    // client, but that will disable all download progress messages by `reqwest` because the
    // gzipped data is decoded on the fly and the size of the decompressed body is unknown.
    // However, we don't really care about the decompressed size but rather we'd like to know
    // the number of raw bytes that are actually downloaded.
    //
    // To do this we manually set the request header to accept gzip encoding and we use the
    // [`AsyncEncoding`] trait to perform the decoding on the fly.
    headers.insert(
        reqwest::header::ACCEPT_ENCODING,
        reqwest::header::HeaderValue::from_static("gzip"),
    );

    // Add previous cache headers if we have them
    if let Some(cache_headers) = cache_state.as_ref().map(|state| &state.cache_headers) {
        cache_headers.add_to_request(&mut headers);
    }
    // Send the request and wait for a reply
    let response = match request_builder.headers(headers).send().await {
        Ok(response) if response.status() == StatusCode::NOT_FOUND => {
            return Err(FetchError::NotFound(DataNotFoundError::from(
                response.error_for_status().unwrap_err(),
            )));
        }
        Ok(response) => response.error_for_status()?,
        Err(e) => {
            return Err(FetchError::HttpError(e));
        }
    };

    // If the content didn't change, simply return whatever we have on disk.
    if response.status() == StatusCode::NOT_MODIFIED {
        tracing::debug!("repodata was unmodified");

        // Update the cache on disk with any new findings.
        let cache_state = CacheState {
            url: data_url,
            has_zst: variant_availability.has_zst,
            has_bz2: variant_availability.has_bz2,
            has_jlap: variant_availability.has_jlap,
            jlap: jlap_state,
            .. cache_state.expect("we must have had a cache, otherwise we wouldn't know the previous state of the cache")
        };

        let cache_state = tokio::task::spawn_blocking(move || {
            cache_state
                .to_path(&cache_state_path)
                .map(|_| cache_state)
                .map_err(FetchError::FailedToWriteCacheState)
        })
        .await??;

        return Ok(CachedData {
            lock_file,
            path: data_json_path,
            cache_state,
            cache_result: CacheResult::CacheHitAfterFetch,
        });
    }

    // Get cache headers from the response
    let cache_headers = CacheHeaders::from(&response);

    // Stream the content to a temporary file
    let (temp_file, blake2_hash) = stream_and_decode_to_file(
        data_url.clone(),
        response,
        if has_zst {
            Encoding::Zst
        } else if has_bz2 {
            Encoding::Bz2
        } else {
            Encoding::Passthrough
        },
        &cache_path,
        progress,
    )
    .await?;

    // Persist the file to its final destination
    let data_destination_path = data_json_path.clone();
    let data_json_metadata = tokio::task::spawn_blocking(move || {
        let file = temp_file
            .persist(data_destination_path)
            .map_err(FetchError::FailedToPersistTemporaryFile)?;

        // Determine the last modified date and size of the data file. We store these values in
        // the cache to link the cache to the corresponding data file.
        file.metadata().map_err(FetchError::FailedToGetMetadata)
    })
    .await??;

    // Update the cache on disk.
    let had_cache = cache_state.is_some();
    let new_cache_state = CacheState {
        url: data_url,
        cache_headers,
        cache_last_modified: data_json_metadata
            .modified()
            .map_err(FetchError::FailedToGetMetadata)?,
        cache_size: data_json_metadata.len(),
        blake2_hash: Some(blake2_hash),
        blake2_hash_nominal: Some(blake2_hash),
        has_zst: variant_availability.has_zst,
        has_bz2: variant_availability.has_bz2,
        has_jlap: variant_availability.has_jlap,
        jlap: jlap_state,
    };

    let new_cache_state = tokio::task::spawn_blocking(move || {
        new_cache_state
            .to_path(&cache_state_path)
            .map(|_| new_cache_state)
            .map_err(FetchError::FailedToWriteCacheState)
    })
    .await??;

    Ok(CachedData {
        lock_file,
        path: data_json_path,
        cache_state: new_cache_state,
        cache_result: if had_cache {
            CacheResult::CacheOutdated
        } else {
            CacheResult::CacheNotPresent
        },
    })
}
