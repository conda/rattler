//! This module provides functionality to cache extracted Conda packages. See
//! [`PackageCache`].

use std::{
    error::Error,
    fmt::Debug,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime},
};

pub use cache_key::CacheKey;
pub use cache_lock::CacheLock;
use cache_lock::CacheRwLock;
use dashmap::DashMap;
use fs_err::tokio as tokio_fs;
use futures::TryFutureExt;
use itertools::Itertools;
use parking_lot::Mutex;
use rattler_conda_types::package::ArchiveIdentifier;
use rattler_digest::Sha256Hash;
use rattler_networking::retry_policies::{DoNotRetryPolicy, RetryDecision, RetryPolicy};
use rattler_package_streaming::{DownloadReporter, ExtractError};
pub use reporter::CacheReporter;
use reqwest::StatusCode;
use simple_spawn_blocking::Cancelled;
use tracing::instrument;
use url::Url;

use crate::validation::{validate_package_directory, ValidationMode};

mod cache_key;
mod cache_lock;
mod reporter;

/// A [`PackageCache`] manages a cache of extracted Conda packages on disk.
///
/// The store does not provide an implementation to get the data into the store.
/// Instead, this is left up to the user when the package is requested. If the
/// package is found in the cache it is returned immediately. However, if the
/// cache is stale a user defined function is called to populate the cache. This
/// separates the concerns between caching and fetching of the content.
#[derive(Clone)]
pub struct PackageCache {
    inner: Arc<PackageCacheInner>,
}

#[derive(Default)]
struct PackageCacheInner {
    layers: Vec<PackageCacheLayer>,
}

pub struct PackageCacheLayer {
    path: PathBuf,
    packages: DashMap<BucketKey, Arc<tokio::sync::Mutex<Entry>>>,
}

/// A key that defines the actual location of the package in the cache.
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct BucketKey {
    name: String,
    version: String,
    build_string: String,
}

impl From<CacheKey> for BucketKey {
    fn from(key: CacheKey) -> Self {
        Self {
            name: key.name,
            version: key.version,
            build_string: key.build_string,
        }
    }
}

#[derive(Default, Debug)]
struct Entry {
    last_revision: Option<u64>,
    last_sha256: Option<Sha256Hash>,
}

/// Errors specific to the PackageCache interface
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PackageCacheError {
    #[error("the operation was cancelled")]
    Cancelled,

    #[error("failed to interact with the package cache layer.")]
    LayerError(#[source] Box<dyn std::error::Error + Send + Sync>), // Wraps layer-specific errors

    #[error("no writable layers to install package to")]
    NoWritableLayers,
}

/// Errors specific to individual layers in the PackageCache
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PackageCacheLayerError {
    #[error("package is invalid")]
    InvalidPackage,

    #[error("package not found in this layer")]
    PackageNotFound,

    #[error("{0}")]
    LockError(String, #[source] std::io::Error),

    #[error("the operation was cancelled")]
    Cancelled,

    #[error(transparent)]
    FetchError(#[from] Arc<dyn std::error::Error + Send + Sync + 'static>),

    #[error("package cache layer error: {0}")]
    OtherError(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl From<Cancelled> for PackageCacheError {
    fn from(_value: Cancelled) -> Self {
        Self::Cancelled
    }
}

impl From<Cancelled> for PackageCacheLayerError {
    fn from(_value: Cancelled) -> Self {
        Self::Cancelled
    }
}

impl From<PackageCacheLayerError> for PackageCacheError {
    fn from(err: PackageCacheLayerError) -> Self {
        // Convert the PackageCacheLayerError to a LayerError by boxing it
        PackageCacheError::LayerError(Box::new(err))
    }
}

impl PackageCacheLayer {
    /// Determine if the layer is read-only in the filesystem
    pub fn is_readonly(&self) -> bool {
        self.path
            .metadata()
            .map(|m| m.permissions().readonly())
            .unwrap_or(false)
    }

    /// Validates a package in a read-only cache.
    /// Acquires a read lock, validates, and returns the lock if valid.
    /// Returns `InvalidPackageInReadOnlyLayer` if validation fails.
    pub async fn validate_or_throw(
        &self,
        cache_key: &CacheKey,
    ) -> Result<CacheLock, PackageCacheLayerError> {
        let cache_entry = self
            .packages
            .get(&cache_key.clone().into())
            .ok_or(PackageCacheLayerError::PackageNotFound)?
            .clone();
        let mut cache_entry = cache_entry.lock().await;
        let cache_path = self.path.join(cache_key.to_string());

        match validate_package_common::<
            fn(PathBuf) -> _,
            Pin<Box<dyn Future<Output = Result<(), _>> + Send>>,
            std::io::Error,
        >(
            cache_path,
            cache_entry.last_revision,
            cache_key.sha256.as_ref(),
            None,
            None,
        )
        .await
        {
            Ok(cache_lock) => {
                cache_entry.last_revision = Some(cache_lock.revision);
                cache_entry.last_sha256 = cache_lock.sha256;
                Ok(cache_lock)
            }
            Err(err) => Err(err),
        }
    }

    /// Validate the package, and fetch it if invalid.
    pub async fn validate_or_fetch<F, Fut, E>(
        &self,
        fetch: F,
        cache_key: &CacheKey,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<CacheLock, PackageCacheLayerError>
    where
        F: (Fn(PathBuf) -> Fut) + Send + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let entry = self
            .packages
            .entry(cache_key.clone().into())
            .or_default()
            .clone();

        let mut cache_entry = entry.lock().await;
        let cache_path = self.path.join(cache_key.to_string());

        match validate_package_common(
            cache_path,
            cache_entry.last_revision,
            cache_key.sha256.as_ref(),
            Some(fetch),
            reporter,
        )
        .await
        {
            Ok(cache_lock) => {
                cache_entry.last_revision = Some(cache_lock.revision);
                cache_entry.last_sha256 = cache_lock.sha256;
                Ok(cache_lock)
            }
            Err(e) => Err(e.into()),
        }
    }
}

impl PackageCache {
    /// Constructs a new [`PackageCache`] with only one layer.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::new_layered(std::iter::once(path.into()))
    }

    /// Constructs a new [`PackageCache`] located at the specified paths.
    /// Read-only layers are queried first.
    /// Within read-only layers, the ordering is defined in this constructor. Ditto for writable layers.
    pub fn new_layered<I>(paths: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<PathBuf>,
    {
        let layers = paths
            .into_iter()
            .map(|path| PackageCacheLayer {
                path: path.into(),
                packages: DashMap::default(),
            })
            .collect();

        Self {
            inner: Arc::new(PackageCacheInner { layers }),
        }
    }

    /// Returns a tuple containing two sets of layers:
    /// - A collection of read-only layers.
    /// - A collection of writable layers.
    ///
    /// The permissions are checked at the time of the function call.
    pub fn split_layers(&self) -> (Vec<&PackageCacheLayer>, Vec<&PackageCacheLayer>) {
        let readonly_layers = self
            .inner
            .layers
            .iter()
            .filter(|layer| layer.is_readonly())
            .collect();
        let writable_layers = self
            .inner
            .layers
            .iter()
            .filter(|layer| !layer.is_readonly())
            .collect();
        (readonly_layers, writable_layers)
    }

    /// Returns the directory that contains the specified package.
    ///
    /// If the package was previously successfully fetched and stored in the
    /// cache the directory containing the data is returned immediately. If
    /// the package was not previously fetch the filesystem is checked to
    /// see if a directory with valid package content exists. Otherwise, the
    /// user provided `fetch` function is called to populate the cache.
    ///
    /// If the package is already being fetched by another task/thread the
    /// request is coalesced. No duplicate fetch is performed.
    pub async fn get_or_fetch<F, Fut, E>(
        &self,
        pkg: impl Into<CacheKey>,
        fetch: F,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<CacheLock, PackageCacheError>
    where
        F: (Fn(PathBuf) -> Fut) + Send + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let cache_key = pkg.into();
        let (readonly_layers, writable_layers) = self.split_layers();
        let to_read = readonly_layers
            .into_iter()
            .chain(writable_layers.clone().into_iter());

        for layer in to_read {
            let cache_path = layer.path.join(cache_key.to_string());

            if cache_path.exists() {
                match layer.validate_or_throw(&cache_key).await {
                    Ok(lock) => {
                        return Ok(lock);
                    }
                    Err(PackageCacheLayerError::InvalidPackage) => {
                        // Log and continue to the next layer
                        tracing::warn!(
                            "Invalid package in layer at path {:?}, trying next layer.",
                            layer.path
                        );
                        continue;
                    }
                    Err(PackageCacheLayerError::PackageNotFound) => {
                        // Log and continue to the next layer
                        tracing::debug!(
                            "Package not found in layer at path {:?}, trying next layer.",
                            layer.path
                        );
                        continue;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }

        // No matches in all layers, let's write to the first writable layer
        tracing::debug!("no matches in all layers. writing to first writable layer");
        if let Some(layer) = writable_layers.get(0) {
            return match layer.validate_or_fetch(fetch, &cache_key, reporter).await {
                Ok(cache_lock) => Ok(cache_lock),
                Err(e) => Err(e.into()),
            };
        }

        Err(PackageCacheError::NoWritableLayers)
    }

    /// Returns the directory that contains the specified package.
    ///
    /// This is a convenience wrapper around `get_or_fetch` which fetches the
    /// package from the given URL if the package could not be found in the
    /// cache.
    pub async fn get_or_fetch_from_url(
        &self,
        pkg: impl Into<CacheKey>,
        url: Url,
        client: reqwest_middleware::ClientWithMiddleware,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<CacheLock, PackageCacheError> {
        self.get_or_fetch_from_url_with_retry(pkg, url, client, DoNotRetryPolicy, reporter)
            .await
    }

    /// Returns the directory that contains the specified package.
    ///
    /// This is a convenience wrapper around `get_or_fetch` which fetches the
    /// package from the given path if the package could not be found in the
    /// cache.
    pub async fn get_or_fetch_from_path(
        &self,
        path: &Path,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<CacheLock, PackageCacheError> {
        let path = path.to_path_buf();
        self.get_or_fetch(
            ArchiveIdentifier::try_from_path(&path).unwrap(),
            move |destination| {
                let path = path.clone();
                async move {
                    rattler_package_streaming::tokio::fs::extract(&path, &destination)
                        .await
                        .map(|_| ())
                }
            },
            reporter,
        )
        .await
    }

    /// Returns the directory that contains the specified package.
    ///
    /// This is a convenience wrapper around `get_or_fetch` which fetches the
    /// package from the given URL if the package could not be found in the
    /// cache.
    #[instrument(skip_all, fields(url=%url))]
    pub async fn get_or_fetch_from_url_with_retry(
        &self,
        pkg: impl Into<CacheKey>,
        url: Url,
        client: reqwest_middleware::ClientWithMiddleware,
        retry_policy: impl RetryPolicy + Send + 'static + Clone,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<CacheLock, PackageCacheError> {
        let request_start = SystemTime::now();
        // Convert into cache key
        let cache_key = pkg.into();
        // Sha256 of the expected package
        let sha256 = cache_key.sha256();
        let download_reporter = reporter.clone();
        // Get or fetch the package, using the specified fetch function
        self.get_or_fetch(cache_key, move |destination| {
            let url = url.clone();
            let client = client.clone();
            let retry_policy = retry_policy.clone();
            let download_reporter = download_reporter.clone();
            async move {
                let mut current_try = 0;
                // Retry until the retry policy says to stop
                loop {
                    current_try += 1;
                    tracing::debug!("downloading {} to {}", &url, destination.display());
                    // Extract the package
                    let result = rattler_package_streaming::reqwest::tokio::extract(
                        client.clone(),
                        url.clone(),
                        &destination,
                        sha256,
                        download_reporter.clone().map(|reporter| Arc::new(PassthroughReporter {
                            reporter,
                            index: Mutex::new(None),
                        }) as Arc::<dyn DownloadReporter>),
                    )
                        .await;

                    // Extract any potential error
                    let Err(err) = result else { return Ok(()); };

                    // Only retry on certain errors.
                    if !matches!(
                    &err,
                    ExtractError::IoError(_) | ExtractError::CouldNotCreateDestination(_)
                ) && !matches!(&err, ExtractError::ReqwestError(err) if
                    err.is_timeout() ||
                    err.is_connect() ||
                    err
                        .status()
                        .map_or(false, |status| status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::REQUEST_TIMEOUT)
                ) {
                        return Err(err);
                    }

                    // Determine whether to retry based on the retry policy
                    let execute_after = match retry_policy.should_retry(request_start, current_try) {
                        RetryDecision::Retry { execute_after } => execute_after,
                        RetryDecision::DoNotRetry => return Err(err),
                    };
                    let duration = execute_after.duration_since(SystemTime::now()).unwrap_or(Duration::ZERO);

                    // Wait for a second to let the remote service restore itself. This increases the
                    // chance of success.
                    tracing::warn!(
                        "failed to download and extract {} to {}: {}. Retry #{}, Sleeping {:?} until the next attempt...",
                        &url,
                        destination.display(),
                        err,
                        current_try,
                        duration
                    );
                    tokio::time::sleep(duration).await;
                }
            }
        }, reporter)
            .await
    }
}

/// Shared logic for validating a package.
async fn validate_package_common<F, Fut, E>(
    path: PathBuf,
    known_valid_revision: Option<u64>,
    given_sha: Option<&Sha256Hash>,
    fetch: Option<F>,
    reporter: Option<Arc<dyn CacheReporter>>,
) -> Result<CacheLock, PackageCacheLayerError>
where
    F: Fn(PathBuf) -> Fut + Send,
    Fut: Future<Output = Result<(), E>> + 'static,
    E: Error + Send + Sync + 'static,
{
    // Acquire a read lock on the cache entry. This ensures that no other process is
    // currently writing to the cache.
    let lock_file_path = {
        // Append the `.lock` extension to the cache path to create the lock file path.
        let mut path_str = path.as_os_str().to_owned();
        path_str.push(".lock");
        PathBuf::from(path_str)
    };

    // Ensure the directory containing the lock-file exists.
    if let Some(root_dir) = lock_file_path.parent() {
        tokio_fs::create_dir_all(root_dir)
            .map_err(|e| {
                PackageCacheLayerError::LockError(
                    format!("failed to create cache directory: '{}'", root_dir.display()),
                    e,
                )
            })
            .await?;
    }

    let mut validated_revision = known_valid_revision;

    loop {
        let mut read_lock = CacheRwLock::acquire_read(&lock_file_path).await?;
        let cache_revision = read_lock.read_revision()?;
        let locked_sha256 = read_lock.read_sha256()?;

        let hash_mismatch = match (given_sha, &locked_sha256) {
            (Some(given_hash), Some(locked_sha256)) => given_hash != locked_sha256,
            _ => false,
        };

        let cache_dir_exists = path.is_dir();
        if cache_dir_exists && !hash_mismatch {
            let path_inner = path.clone();

            let reporter = reporter.as_deref().map(|r| (r, r.on_validate_start()));

            // If we know the revision is already valid we can return immediately.
            if validated_revision.map_or(false, |validated_revision| {
                validated_revision == cache_revision
            }) {
                if let Some((reporter, index)) = reporter {
                    reporter.on_validate_complete(index);
                }
                return Ok(CacheLock {
                    _lock: read_lock,
                    revision: cache_revision,
                    sha256: locked_sha256,
                    path: path_inner,
                });
            }

            // Validate the package directory.
            let validation_result = tokio::task::spawn_blocking(move || {
                validate_package_directory(&path_inner, ValidationMode::Fast)
            })
            .await;

            if let Some((reporter, index)) = reporter {
                reporter.on_validate_complete(index);
            }

            match validation_result {
                Ok(Ok(_)) => {
                    tracing::debug!("validation succeeded");
                    return Ok(CacheLock {
                        _lock: read_lock,
                        revision: cache_revision,
                        sha256: locked_sha256,
                        path,
                    });
                }
                Ok(Err(e)) => {
                    tracing::warn!("validation for {path:?} failed: {e}");
                    if let Some(cause) = e.source() {
                        tracing::debug!(
                            "  Caused by: {}",
                            std::iter::successors(Some(cause), |e| (*e).source())
                                .format("\n  Caused by: ")
                        );
                    }
                }
                Err(e) => {
                    if let Ok(panic) = e.try_into_panic() {
                        std::panic::resume_unwind(panic)
                    }
                }
            }
        } else if !cache_dir_exists {
            tracing::debug!("cache directory does not exist");
        } else if hash_mismatch {
            tracing::warn!(
                "hash mismatch, wanted a package with hash {} but the cached package has hash {}",
                given_sha.map_or(String::from("<unknown>"), |s| format!("{s:x}")),
                locked_sha256.map_or(String::from("<unknown>"), |s| format!("{s:x}"))
            );
        }

        // The cache is invalid
        // Refetch, or throw validation error if no fetch function is supplied.
        if let Some(ref fetch_fn) = fetch {
            drop(read_lock);

            let mut write_lock = CacheRwLock::acquire_write(&lock_file_path).await?;

            let read_revision = write_lock.read_revision()?;
            if read_revision != cache_revision {
                tracing::debug!(
                    "cache revisions don't match '{}', retrying to acquire lock file.",
                    lock_file_path.display()
                );
                continue;
            }

            // Write the new revision
            let new_revision = cache_revision + 1;
            write_lock
                .write_revision_and_sha(new_revision, given_sha)
                .await?;

            // Fetch the package.
            fetch_fn(path.clone())
                .await
                .map_err(|e| PackageCacheLayerError::FetchError(Arc::new(e)))?;

            validated_revision = Some(new_revision);
        } else {
            return Err(PackageCacheLayerError::InvalidPackage);
        }
    }
}

struct PassthroughReporter {
    reporter: Arc<dyn CacheReporter>,
    index: Mutex<Option<usize>>,
}

impl DownloadReporter for PassthroughReporter {
    fn on_download_start(&self) {
        let index = self.reporter.on_download_start();
        assert!(
            self.index.lock().replace(index).is_none(),
            "on_download_start was called multiple times"
        );
    }

    fn on_download_progress(&self, bytes_downloaded: u64, total_bytes: Option<u64>) {
        let index = self.index.lock().expect("on_download_start was not called");
        self.reporter
            .on_download_progress(index, bytes_downloaded, total_bytes);
    }

    fn on_download_complete(&self) {
        let index = self
            .index
            .lock()
            .take()
            .expect("on_download_start was not called");
        self.reporter.on_download_completed(index);
    }
}

#[cfg(test)]
mod test {
    use std::{
        convert::Infallible,
        fs::File,
        future::IntoFuture,
        net::SocketAddr,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    use assert_matches::assert_matches;
    use axum::{
        body::Body,
        extract::State,
        http::{Request, StatusCode},
        middleware,
        middleware::Next,
        response::{Redirect, Response},
        routing::get,
        Router,
    };
    use bytes::Bytes;
    use futures::stream;
    use rattler_conda_types::package::{ArchiveIdentifier, PackageFile, PathsJson};
    use rattler_digest::{parse_digest_from_hex, Sha256};
    use rattler_networking::retry_policies::{DoNotRetryPolicy, ExponentialBackoffBuilder};
    use tempfile::{tempdir, TempDir};
    use tokio::sync::Mutex;
    use tokio_stream::StreamExt;
    use url::Url;

    use super::PackageCache;
    use crate::{package_cache::CacheKey, validation::validate_package_directory};
    use crate::{package_cache::PackageCacheError, validation::ValidationMode};

    fn get_test_data_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
    }

    #[tokio::test]
    pub async fn test_package_cache() {
        let tar_archive_path = tools::download_and_cache_file_async("https://conda.anaconda.org/robostack/linux-64/ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2".parse().unwrap(),
                                                                    "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8").await.unwrap();

        // Read the paths.json file straight from the tar file.
        let paths = {
            let tar_reader = File::open(&tar_archive_path).unwrap();
            let mut tar_archive = rattler_package_streaming::read::stream_tar_bz2(tar_reader);
            let tar_entries = tar_archive.entries().unwrap();
            let paths_entry = tar_entries
                .map(Result::unwrap)
                .find(|entry| entry.path().unwrap().as_ref() == Path::new("info/paths.json"))
                .unwrap();
            PathsJson::from_reader(paths_entry).unwrap()
        };

        let packages_dir = tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        // Get the package to the cache
        let cache_lock = cache
            .get_or_fetch(
                ArchiveIdentifier::try_from_path(&tar_archive_path).unwrap(),
                move |destination| {
                    let tar_archive_path = tar_archive_path.clone();
                    async move {
                        rattler_package_streaming::tokio::fs::extract(
                            &tar_archive_path,
                            &destination,
                        )
                        .await
                        .map(|_| ())
                    }
                },
                None,
            )
            .await
            .unwrap();

        // Validate the contents of the package
        let (_, current_paths) =
            validate_package_directory(cache_lock.path(), ValidationMode::Full).unwrap();

        // Make sure that the paths are the same as what we would expect from the
        // original tar archive.
        assert_eq!(current_paths, paths);
    }

    /// A helper middleware function that fails the first two requests.
    async fn fail_the_first_two_requests(
        State(count): State<Arc<Mutex<i32>>>,
        req: Request<Body>,
        next: Next,
    ) -> Result<Response, StatusCode> {
        let count = {
            let mut count = count.lock().await;
            *count += 1;
            *count
        };

        println!("Running middleware for request #{count} for {}", req.uri());
        if count <= 2 {
            println!("Discarding request!");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }

        // requires the http crate to get the header name
        Ok(next.run(req).await)
    }

    /// A helper middleware function that fails the first two requests.
    #[allow(clippy::type_complexity)]
    async fn fail_with_half_package(
        State((count, bytes)): State<(Arc<Mutex<i32>>, Arc<Mutex<usize>>)>,
        req: Request<Body>,
        next: Next,
    ) -> Result<Response, StatusCode> {
        let count = {
            let mut count = count.lock().await;
            *count += 1;
            *count
        };

        println!("Running middleware for request #{count} for {}", req.uri());
        let response = next.run(req).await;

        if count <= 2 {
            // println!("Cutting response body in half");
            let body = response.into_body();
            let mut body = body.into_data_stream();
            let mut buffer = Vec::new();
            while let Some(Ok(chunk)) = body.next().await {
                buffer.extend(chunk);
            }

            let byte_count = *bytes.lock().await;
            let bytes = buffer.into_iter().take(byte_count).collect::<Vec<u8>>();
            // Create a stream that ends prematurely
            let stream = stream::iter(vec![
                Ok::<_, Infallible>(bytes.into_iter().collect::<Bytes>()),
                // The stream ends after sending partial data, simulating a premature close
            ]);
            let body = Body::from_stream(stream);
            return Ok(Response::new(body));
        }

        Ok(response)
    }

    enum Middleware {
        FailTheFirstTwoRequests,
        FailAfterBytes(usize),
    }

    async fn redirect_to_anaconda(
        axum::extract::Path((channel, subdir, file)): axum::extract::Path<(String, String, String)>,
    ) -> Redirect {
        Redirect::permanent(&format!(
            "https://conda.anaconda.org/{channel}/{subdir}/{file}"
        ))
    }

    async fn test_flaky_package_cache(archive_name: &str, middleware: Middleware) {
        let static_dir = get_test_data_dir();
        println!("Serving files from {}", static_dir.display());

        // Construct a service that serves raw files from the test directory
        // build our application with a route
        let router = Router::new()
            // `GET /` goes to `root`
            .route("/:channel/:subdir/:file", get(redirect_to_anaconda));

        // Construct a router that returns data from the static dir but fails the first
        // try.
        let request_count = Arc::new(Mutex::new(0));

        let router = match middleware {
            Middleware::FailTheFirstTwoRequests => router.layer(middleware::from_fn_with_state(
                request_count.clone(),
                fail_the_first_two_requests,
            )),
            Middleware::FailAfterBytes(size) => router.layer(middleware::from_fn_with_state(
                (request_count.clone(), Arc::new(Mutex::new(size))),
                fail_with_half_package,
            )),
        };

        // Construct the server that will listen on localhost but with a *random port*.
        // The random port is very important because it enables creating
        // multiple instances at the same time. We need this to be able to run
        // tests in parallel.
        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let service = router.into_make_service();
        tokio::spawn(axum::serve(listener, service).into_future());

        let packages_dir = tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        let server_url = Url::parse(&format!("http://localhost:{}", addr.port())).unwrap();

        // Do the first request without
        let result = cache
            .get_or_fetch_from_url_with_retry(
                ArchiveIdentifier::try_from_filename(archive_name).unwrap(),
                server_url.join(archive_name).unwrap(),
                reqwest::Client::default().into(),
                DoNotRetryPolicy,
                None,
            )
            .await;

        // First request without retry policy should fail
        assert_matches!(result, Err(_));
        {
            let request_count_lock = request_count.lock().await;
            assert_eq!(*request_count_lock, 1, "Expected there to be 1 request");
        }

        // The second one should fail after the 2nd try
        let result = cache
            .get_or_fetch_from_url_with_retry(
                ArchiveIdentifier::try_from_filename(archive_name).unwrap(),
                server_url.join(archive_name).unwrap(),
                reqwest::Client::default().into(),
                ExponentialBackoffBuilder::default().build_with_max_retries(3),
                None,
            )
            .await;

        assert!(result.is_ok());
        {
            let request_count_lock = request_count.lock().await;
            assert_eq!(*request_count_lock, 3, "Expected there to be 3 requests");
        }
    }

    #[tokio::test]
    async fn test_flaky() {
        let tar_bz2 = "conda-forge/win-64/conda-22.9.0-py310h5588dad_2.tar.bz2";
        let conda = "conda-forge/win-64/conda-22.11.1-py38haa244fe_1.conda";

        test_flaky_package_cache(tar_bz2, Middleware::FailTheFirstTwoRequests).await;
        test_flaky_package_cache(conda, Middleware::FailTheFirstTwoRequests).await;

        test_flaky_package_cache(tar_bz2, Middleware::FailAfterBytes(1000)).await;
        test_flaky_package_cache(conda, Middleware::FailAfterBytes(1000)).await;
        test_flaky_package_cache(conda, Middleware::FailAfterBytes(50)).await;
    }

    #[tokio::test]
    async fn test_multi_process() {
        let packages_dir = tempdir().unwrap();
        let cache_a = PackageCache::new(packages_dir.path());
        let cache_b = PackageCache::new(packages_dir.path());
        let cache_c = PackageCache::new(packages_dir.path());

        let package_path = get_test_data_dir().join("clobber/clobber-python-0.1.0-cpython.conda");

        // Get the file to the cache
        let cache_a_lock = cache_a
            .get_or_fetch_from_path(&package_path, None)
            .await
            .unwrap();

        assert_eq!(cache_a_lock.revision(), 1);

        // Get the file to the cache
        let cache_b_lock = cache_b
            .get_or_fetch_from_path(&package_path, None)
            .await
            .unwrap();

        assert_eq!(cache_b_lock.revision(), 1);

        // Now delete the index.json from the cache entry, effectively
        // corrupting the cache.
        std::fs::remove_file(cache_a_lock.path().join("info/index.json")).unwrap();

        // Drop previous locks to ensure the package is not locked.
        drop(cache_a_lock);
        drop(cache_b_lock);

        // Get the file to the cache
        let cache_c_lock = cache_c
            .get_or_fetch_from_path(&package_path, None)
            .await
            .unwrap();

        assert_eq!(cache_c_lock.revision(), 2);
    }

    #[tokio::test]
    // Test if packages with different sha's are replaced even though they share the
    // same BucketKey.
    pub async fn test_package_cache_key_with_sha() {
        let tar_archive_path = tools::download_and_cache_file_async("https://conda.anaconda.org/robostack/linux-64/ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2".parse().unwrap(), "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8").await.unwrap();

        // Create a temporary directory to store the packages
        let packages_dir = tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        // Set the sha256 of the package
        let key: CacheKey = ArchiveIdentifier::try_from_path(&tar_archive_path)
            .unwrap()
            .into();
        let key = key.with_sha256(
            parse_digest_from_hex::<Sha256>(
                "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8",
            )
            .unwrap(),
        );

        // Get the package to the cache
        let cloned_archive_path = tar_archive_path.clone();
        let cache_lock = cache
            .get_or_fetch(
                key.clone(),
                move |destination| {
                    let cloned_archive_path = cloned_archive_path.clone();
                    async move {
                        rattler_package_streaming::tokio::fs::extract(
                            &cloned_archive_path,
                            &destination,
                        )
                        .await
                        .map(|_| ())
                    }
                },
                None,
            )
            .await
            .unwrap();

        let sha_1 = cache_lock.sha256.expect("expected sha256 to be set");
        drop(cache_lock);

        let new_sha = parse_digest_from_hex::<Sha256>(
            "5dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc9",
        )
        .unwrap();
        let key = key.with_sha256(new_sha);
        // Change the sha256 of the package
        // And expect the package to be replaced
        let should_run = Arc::new(AtomicBool::new(false));
        let cloned = should_run.clone();
        let cache_lock = cache
            .get_or_fetch(
                key.clone(),
                move |destination| {
                    let tar_archive_path = tar_archive_path.clone();
                    cloned.store(true, Ordering::Release);
                    async move {
                        rattler_package_streaming::tokio::fs::extract(
                            &tar_archive_path,
                            &destination,
                        )
                        .await
                        .map(|_| ())
                    }
                },
                None,
            )
            .await
            .unwrap();
        assert!(
            should_run.load(Ordering::Relaxed),
            "fetch function should run again"
        );
        assert_ne!(
            sha_1,
            cache_lock.sha256.expect("expected sha256 to be set"),
            "expected sha256 to be different"
        );
    }

    #[derive(Debug)]
    pub struct PackageInstallInfo {
        pub url: Url,
        // is_readonly=true and layer_num=0 means this package will be installed to the first readonly cache layer
        pub is_readonly: bool,
        pub layer_num: usize,
        pub expected_sha: String,
    }

    /// A helper function to create a layered cache, and install packages to specific layers
    async fn create_layered_cache(
        readonly_layer_count: usize,
        writable_layer_count: usize,
        packages: Vec<PackageInstallInfo>, // Use the new struct
    ) -> (PackageCache, Vec<TempDir>) {
        let mut readonly_dirs = Vec::new();
        let mut writable_dirs = Vec::new();

        for _ in 0..readonly_layer_count {
            readonly_dirs.push(tempdir().unwrap());
        }

        for _ in 0..writable_layer_count {
            writable_dirs.push(tempdir().unwrap());
        }

        let all_layers_paths: Vec<TempDir> = readonly_dirs
            .into_iter()
            .chain(writable_dirs.into_iter())
            .collect();

        let cache =
            PackageCache::new_layered(all_layers_paths.iter().map(|dir| dir.path().to_path_buf()));

        let (readonly_layers, writable_layers) = cache.inner.layers.split_at(readonly_layer_count);

        // Install the packages to the appropriate layers
        for package in packages {
            let layer = if package.is_readonly {
                &readonly_layers[package.layer_num]
            } else {
                &writable_layers[package.layer_num]
            };
            let tar_archive_path =
                tools::download_and_cache_file_async(package.url, &package.expected_sha)
                    .await
                    .unwrap();

            let key: CacheKey = ArchiveIdentifier::try_from_path(&tar_archive_path)
                .unwrap()
                .into();
            let key =
                key.with_sha256(parse_digest_from_hex::<Sha256>(&package.expected_sha).unwrap());

            layer
                .validate_or_fetch(
                    move |destination| {
                        let tar_archive_path = tar_archive_path.clone();
                        async move {
                            rattler_package_streaming::tokio::fs::extract(
                                &tar_archive_path,
                                &destination,
                            )
                            .await
                            .map(|_| ())
                        }
                    },
                    &key,
                    None,
                )
                .await
                .unwrap();
        }

        for layer in readonly_layers {
            #[cfg(unix)]
            std::fs::set_permissions(
                &layer.path,
                std::os::unix::fs::PermissionsExt::from_mode(0o555), // r_x r_x r_x
            )
            .unwrap();
        }
        (cache, all_layers_paths)
    }

    #[tokio::test]
    async fn test_package_only_in_readonly() {
        // Create one readonly layer and one writable layer, and install the package to the readonly layer
        let url: Url =  "https://conda.anaconda.org/robostack/linux-64/ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2".parse().unwrap();
        let sha = "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8".to_string();
        let (cache, _dirs) = create_layered_cache(
            1,
            1,
            vec![PackageInstallInfo {
                url: url.clone(),
                is_readonly: true,
                layer_num: 0,
                expected_sha: sha.clone(),
            }],
        )
        .await;

        let cache_key = CacheKey::from(ArchiveIdentifier::try_from_url(&url).unwrap());
        let cache_key = cache_key.with_sha256(parse_digest_from_hex::<Sha256>(&sha).unwrap());

        let should_run = Arc::new(AtomicBool::new(false));
        let cloned = should_run.clone();

        // Fetch function shouldn't run
        cache
            .get_or_fetch(
                cache_key.clone(),
                move |_destination| {
                    cloned.store(true, Ordering::Relaxed);
                    async { Ok::<_, PackageCacheError>(()) }
                },
                None,
            )
            .await
            .unwrap();

        assert!(
            !should_run.load(Ordering::Relaxed),
            "fetch function should not be run"
        );
    }

    #[tokio::test]
    async fn test_package_only_in_writable() {
        // Create one readonly layer and one writable layer, and install the package to the readonly layer
        let url: Url =  "https://conda.anaconda.org/robostack/linux-64/ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2".parse().unwrap();
        let sha = "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8".to_string();
        let (cache, _dirs) = create_layered_cache(
            1,
            1,
            vec![PackageInstallInfo {
                url: url.clone(),
                is_readonly: false,
                layer_num: 0,
                expected_sha: sha.clone(),
            }],
        )
        .await;

        let cache_key = CacheKey::from(ArchiveIdentifier::try_from_url(&url).unwrap());
        let cache_key = cache_key.with_sha256(parse_digest_from_hex::<Sha256>(&sha).unwrap());

        let should_run = Arc::new(AtomicBool::new(false));
        let cloned = should_run.clone();

        // Fetch function shouldn't run
        cache
            .get_or_fetch(
                cache_key.clone(),
                move |_destination| {
                    cloned.store(true, Ordering::Relaxed);
                    async { Ok::<_, PackageCacheError>(()) }
                },
                None,
            )
            .await
            .unwrap();

        assert!(
            !should_run.load(Ordering::Relaxed),
            "fetch function should not be run"
        );
    }

    #[tokio::test]
    async fn test_package_not_in_any_layer() {
        // Create one readonly layer and one writable layer, and install a package to the readonly layer
        let url: Url =  "https://conda.anaconda.org/robostack/linux-64/ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2".parse().unwrap();
        let sha = "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8".to_string();
        let (cache, _dirs) = create_layered_cache(
            1,
            1,
            vec![PackageInstallInfo {
                url: url.clone(),
                is_readonly: true,
                layer_num: 0,
                expected_sha: sha.clone(),
            }],
        )
        .await;

        // Request a different package, not installed in any layer
        let other_url: Url =
            "https://conda.anaconda.org/conda-forge/win-64/mamba-1.1.0-py39hb3d9227_2.conda"
                .parse()
                .unwrap();
        let other_sha =
            "c172acdf9cb7655dd224879b30361a657b09bb084b65f151e36a2b51e51a080a".to_string();

        let cache_key = CacheKey::from(ArchiveIdentifier::try_from_url(&other_url).unwrap());
        let cache_key = cache_key.with_sha256(parse_digest_from_hex::<Sha256>(&other_sha).unwrap());

        let should_run = Arc::new(AtomicBool::new(false));
        let cloned = should_run.clone();

        let tar_archive_path = tools::download_and_cache_file_async(other_url, &other_sha)
            .await
            .unwrap();

        // The fetch function should run
        cache
            .get_or_fetch(
                cache_key.clone(),
                move |destination| {
                    let tar_archive_path = tar_archive_path.clone();
                    cloned.store(true, Ordering::Release);
                    async move {
                        rattler_package_streaming::tokio::fs::extract(
                            &tar_archive_path,
                            &destination,
                        )
                        .await
                        .map(|_| ())
                    }
                },
                None,
            )
            .await
            .unwrap();

        assert!(
            should_run.load(Ordering::Relaxed),
            "fetch function should run again"
        );
    }
}
