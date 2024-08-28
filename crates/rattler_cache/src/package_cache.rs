//! This module provides functionality to cache extracted Conda packages. See
//! [`PackageCache`].

use std::{
    error::Error,
    fmt::{Debug, Display, Formatter},
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use async_fd_lock::{LockRead, LockWrite, RwLockReadGuard, RwLockWriteGuard};
use dashmap::DashMap;
use futures::TryFutureExt;
use itertools::Itertools;
use parking_lot::Mutex;
use rattler_conda_types::{package::ArchiveIdentifier, PackageRecord};
use rattler_digest::Sha256Hash;
use rattler_networking::retry_policies::{DoNotRetryPolicy, RetryDecision, RetryPolicy};
use rattler_package_streaming::{DownloadReporter, ExtractError};
use reqwest::StatusCode;
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
use url::Url;

use crate::validation::validate_package_directory;

/// A trait that can be implemented to report progress of the download and
/// validation process.
pub trait CacheReporter: Send + Sync {
    /// Called when validation starts
    fn on_validate_start(&self) -> usize;
    /// Called when validation completex
    fn on_validate_complete(&self, index: usize);
    /// Called when a download starts
    fn on_download_start(&self) -> usize;
    /// Called with regular updates on the download progress
    fn on_download_progress(&self, index: usize, progress: u64, total: Option<u64>);
    /// Called when a download completes
    fn on_download_completed(&self, index: usize);
}

/// A [`PackageCache`] manages a cache of extracted Conda packages on disk.
///
/// The store does not provide an implementation to get the data into the store.
/// Instead this is left up to the user when the package is requested. If the
/// package is found in the cache it is returned immediately. However, if the
/// cache is stale a user defined function is called to populate the cache. This
/// separates the corners between caching and fetching of the content.
#[derive(Clone)]
pub struct PackageCache {
    inner: Arc<PackageCacheInner>,
}

/// Provides a unique identifier for packages in the cache.
/// TODO: This could not be unique over multiple subdir. How to handle?
/// TODO: Wouldn't it be better to cache based on hashes?
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct CacheKey {
    name: String,
    version: String,
    build_string: String,
    sha256: Option<Sha256Hash>,
}

impl CacheKey {
    /// Adds a sha256 hash of the archive.
    pub fn with_sha256(mut self, sha256: Sha256Hash) -> Self {
        self.sha256 = Some(sha256);
        self
    }

    /// Potentially adds a sha256 hash of the archive.
    pub fn with_opt_sha256(mut self, sha256: Option<Sha256Hash>) -> Self {
        self.sha256 = sha256;
        self
    }
}

impl CacheKey {
    /// Return the sha256 hash of the package if it is known.
    pub fn sha256(&self) -> Option<Sha256Hash> {
        self.sha256
    }
}

impl From<ArchiveIdentifier> for CacheKey {
    fn from(pkg: ArchiveIdentifier) -> Self {
        CacheKey {
            name: pkg.name,
            version: pkg.version,
            build_string: pkg.build_string,
            sha256: None,
        }
    }
}

impl From<&PackageRecord> for CacheKey {
    fn from(record: &PackageRecord) -> Self {
        Self {
            name: record.name.as_normalized().to_string(),
            version: record.version.to_string(),
            build_string: record.build.clone(),
            sha256: record.sha256,
        }
    }
}

impl Display for CacheKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}-{}", &self.name, &self.version, &self.build_string)
    }
}

#[derive(Default)]
struct PackageCacheInner {
    path: PathBuf,
    packages: DashMap<CacheKey, tokio::sync::Mutex<Entry>>,
}

#[derive(Default)]
struct Entry {
    last_revision: Option<u64>,
}

/// An error that might be returned from one of the caching function of the
/// [`PackageCache`].
#[derive(Debug, thiserror::Error)]
pub enum PackageCacheError {
    /// An error occurred while fetching the package.
    #[error(transparent)]
    FetchError(#[from] Arc<dyn std::error::Error + Send + Sync + 'static>),

    /// A locking error occured
    #[error("{0}")]
    LockError(String, #[source] std::io::Error),
}

impl PackageCache {
    /// Constructs a new [`PackageCache`] located at the specified path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(PackageCacheInner {
                path: path.into(),
                packages: DashMap::default(),
            }),
        }
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
        let cache_path = self.inner.path.join(cache_key.to_string());
        let cache_entry = self.inner.packages.entry(cache_key).or_default();

        // Acquire the entry. From this point on we can be sure that only one task is
        // accessing the cache entry.
        let mut cache_entry = cache_entry.lock().await;

        // Validate the cache entry or fetch the package if it is not valid.
        let cache_lock =
            validate_or_fetch_to_cache(cache_path, fetch, cache_entry.last_revision, reporter)
                .await?;

        // Store the current revision stored in the cache. If any other task tries to
        // read the cache and the revision stayed the same, we can assume that the cache
        // is still valid.
        cache_entry.last_revision = Some(cache_lock.revision);

        Ok(cache_lock)
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
    pub async fn get_or_fetch_from_url_with_retry(
        &self,
        pkg: impl Into<CacheKey>,
        url: Url,
        client: reqwest_middleware::ClientWithMiddleware,
        retry_policy: impl RetryPolicy + Send + 'static + Clone,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<CacheLock, PackageCacheError> {
        let request_start = SystemTime::now();
        let cache_key = pkg.into();
        let sha256 = cache_key.sha256();
        let download_reporter = reporter.clone();
        self.get_or_fetch(cache_key, move |destination| {
            let url = url.clone();
            let client = client.clone();
            let retry_policy = retry_policy.clone();
            let download_reporter = download_reporter.clone();
            async move {
                let mut current_try = 0;
                loop {
                    current_try += 1;
                    tracing::debug!("downloading {} to {}", &url, destination.display());

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

/// A lock on the cache entry. As long as this lock is held, no other process is
/// allowed to modify the cache entry. This however, does not guarantee that the
/// contents of the cache is not corrupted by external processes, but it does
/// guarantee that when concurrent processes access the package cache they do
/// not interfere with each other.
pub struct CacheLock {
    _lock: CacheRwLock<RwLockReadGuard<File>>,
    revision: u64,
    path: PathBuf,
}

impl Debug for CacheLock {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheLock")
            .field("path", &self.path)
            .field("revision", &self.revision)
            .finish()
    }
}

impl CacheLock {
    /// Returns the path to the cache entry on disk.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the revision of the cache entry. This revision indicates the
    /// number of times the cache entry has been updated.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

struct CacheRwLock<L> {
    lock: L,
}

impl CacheRwLock<RwLockReadGuard<File>> {
    pub async fn acquire_read(path: &Path) -> Result<Self, PackageCacheError> {
        let lock_file_path = path.to_path_buf();
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .await
            .map_err(|e| {
                PackageCacheError::LockError(
                    format!(
                        "failed to open cache lock for reading: '{}",
                        lock_file_path.display()
                    ),
                    e,
                )
            })?;

        let lock_file_path = path.to_path_buf();
        let acquire_lock_fut = file.lock_read().map_err(move |e| {
            PackageCacheError::LockError(
                format!(
                    "failed to acquire read lock on cache lock file: '{}'",
                    lock_file_path.display()
                ),
                e.error,
            )
        });

        tokio::select!(
            lock = acquire_lock_fut => Ok(CacheRwLock { lock: lock? }),
            _ = warn_timeout_future(format!(
                "Blocking waiting for file lock on package cache for {}",
                path.file_name()
                    .expect("lock file must have a name")
                    .to_string_lossy()
            )) => unreachable!("warn_timeout_future should never finish")
        )
    }
}

impl CacheRwLock<RwLockWriteGuard<File>> {
    pub async fn acquire_write(path: &Path) -> Result<Self, PackageCacheError> {
        let lock_file_path = path.to_path_buf();
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&path)
            .await
            .map_err(|e| {
                PackageCacheError::LockError(
                    format!(
                        "failed to open cache lock for writing: '{}",
                        lock_file_path.display()
                    ),
                    e,
                )
            })?;

        let lock_file_path = path.to_path_buf();
        let acquire_lock_fut = file.lock_write().map_err(move |e| {
            PackageCacheError::LockError(
                format!(
                    "failed to acquire write lock on cache lock file: '{}'",
                    lock_file_path.display()
                ),
                e.error,
            )
        });

        tokio::select!(
            lock = acquire_lock_fut => Ok(CacheRwLock { lock: lock? }),
            _ = warn_timeout_future(format!(
                "Blocking waiting for file lock on package cache for {}",
                path.file_name()
                    .expect("lock file must have a name")
                    .to_string_lossy()
            )) => unreachable!("warn_timeout_future should never finish")
        )
    }
}

impl CacheRwLock<RwLockWriteGuard<File>> {
    async fn write_revision(&mut self, revision: u64) -> Result<(), PackageCacheError> {
        // Ensure we write from the start of the file
        self.lock.inner_mut().rewind().await.map_err(|e| {
            PackageCacheError::LockError(
                "failed to rewind cache lock for reading revision".to_string(),
                e,
            )
        })?;

        // Write the bytes of the revision
        let revision_bytes = revision.to_be_bytes();
        self.lock.write_all(&revision_bytes).await.map_err(|e| {
            PackageCacheError::LockError("failed to write revision from cache lock".to_string(), e)
        })?;

        // Ensure all bytes are written to disk
        self.lock.flush().await.map_err(|e| {
            PackageCacheError::LockError(
                "failed to flush cache lock after writing revision".to_string(),
                e,
            )
        })?;

        // Update the length of the file
        self.lock
            .inner_mut()
            .set_len(revision_bytes.len() as u64)
            .await
            .map_err(|e| {
                PackageCacheError::LockError(
                    "failed to truncate cache lock after writing revision".to_string(),
                    e,
                )
            })?;

        Ok(())
    }
}

impl<R: AsyncRead + Unpin> CacheRwLock<R> {
    async fn read_revision(&mut self) -> Result<u64, PackageCacheError> {
        let mut buf = [0; 8];
        match self.lock.read_exact(&mut buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(0);
            }
            Err(e) => {
                return Err(PackageCacheError::LockError(
                    "failed to read revision from cache lock".to_string(),
                    e,
                ));
            }
        }
        Ok(u64::from_be_bytes(buf))
    }
}

async fn warn_timeout_future(message: String) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        tracing::warn!("{}", &message);
    }
}

/// Validates that the package that is currently stored is a valid package and
/// otherwise calls the `fetch` method to populate the cache.
async fn validate_or_fetch_to_cache<F, Fut, E>(
    path: PathBuf,
    fetch: F,
    known_valid_revision: Option<u64>,
    reporter: Option<Arc<dyn CacheReporter>>,
) -> Result<CacheLock, PackageCacheError>
where
    F: Fn(PathBuf) -> Fut + Send,
    Fut: Future<Output = Result<(), E>> + 'static,
    E: Error + Send + Sync + 'static,
{
    // Acquire a read lock on the cache entry. This ensures that no other process is
    // currently writing to the cache.
    let lock_file_path = path.join(".lock");

    if let Some(root_dir) = lock_file_path.parent() {
        tokio::fs::create_dir_all(root_dir)
            .map_err(|e| {
                PackageCacheError::LockError(
                    format!("failed to create cache directory: '{}", root_dir.display()),
                    e,
                )
            })
            .await?;
    }

    // The revision of the cache entry that we already know is valid.
    let mut validated_revision = known_valid_revision;

    loop {
        let mut read_lock = CacheRwLock::acquire_read(&lock_file_path).await?;
        let cache_revision = read_lock.read_revision().await?;

        if path.is_dir() {
            let path_inner = path.clone();

            let reporter = reporter.as_deref().map(|r| (r, r.on_validate_start()));

            match validated_revision {
                Some(revision) if revision == cache_revision => {
                    // We previously already determined that the revision is valid. We can skip
                    // actually validating.
                    if let Some((reporter, index)) = reporter {
                        reporter.on_validate_complete(index);
                    }
                    return Ok(CacheLock {
                        _lock: read_lock,
                        revision: cache_revision,
                        path: path_inner,
                    });
                }
                Some(_) => {
                    // The cache has been modified since the last validation. We need to
                    // re-validate.
                    tracing::debug!(
                        "cache became stale while acquiring a read-lock from {}. Revalidating.",
                        lock_file_path.display()
                    );
                }
                None => {
                    // We have no information about the cache revision. We need
                    // to validate.
                }
            }

            let validation_result =
                tokio::task::spawn_blocking(move || validate_package_directory(&path_inner)).await;

            if let Some((reporter, index)) = reporter {
                reporter.on_validate_complete(index);
            }

            match validation_result {
                Ok(Ok(_)) => {
                    tracing::debug!("validation succeeded");
                    return Ok(CacheLock {
                        _lock: read_lock,
                        revision: cache_revision,
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
        }

        // If the cache is stale, we need to fetch the package again. We have to acquire
        // a write lock on the cache entry. However, we can't do that while we have a
        // read lock on the cache lock file. So we release the read lock and acquire a
        // write lock on the cache lock file. In the meantime, another process might
        // have already fetched the package. To guard against this we read a revision
        // from the lock-file while we have the read lock, then we acquire the write
        // lock and check if the revision has changed. If it has, we assume that
        // another process has already fetched the package and we restart the
        // validation process.

        drop(read_lock);

        let mut write_lock = CacheRwLock::acquire_write(&lock_file_path).await?;
        let read_revision = write_lock.read_revision().await?;
        if read_revision != cache_revision {
            // The cache has been modified since we last checked. We need to re-validate.
            continue;
        }

        // Write the new revision
        let new_revision = cache_revision + 1;
        write_lock.write_revision(new_revision).await?;

        // Otherwise, defer to populate method to fill our cache.
        fetch(path.clone())
            .await
            .map_err(|e| PackageCacheError::FetchError(Arc::new(e)))?;

        validated_revision = Some(new_revision);
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
        sync::Arc,
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
    use rattler_networking::retry_policies::{DoNotRetryPolicy, ExponentialBackoffBuilder};
    use tempfile::tempdir;
    use tokio::sync::Mutex;
    use tokio_stream::StreamExt;
    use url::Url;

    use super::PackageCache;
    use crate::validation::validate_package_directory;

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
        let (_, current_paths) = validate_package_directory(cache_lock.path()).unwrap();

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
}
