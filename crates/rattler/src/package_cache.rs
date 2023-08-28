//! This module provides functionality to cache extracted Conda packages. See [`PackageCache`].

use crate::validation::validate_package_directory;
use chrono::Utc;
use fxhash::FxHashMap;
use itertools::Itertools;
use rattler_conda_types::{package::ArchiveIdentifier, PackageRecord};
use rattler_networking::{
    retry_policies::{DoNotRetryPolicy, RetryDecision, RetryPolicy},
    AuthenticatedClient,
};
use rattler_package_streaming::ExtractError;
use reqwest::StatusCode;
use std::error::Error;
use std::{
    fmt::{Display, Formatter},
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;
use tracing::Instrument;
use url::Url;

/// A [`PackageCache`] manages a cache of extracted Conda packages on disk.
///
/// The store does not provide an implementation to get the data into the store. Instead this is
/// left up to the user when the package is requested. If the package is found in the cache it is
/// returned immediately. However, if the cache is stale a user defined function is called to
/// populate the cache. This separates the corners between caching and fetching of the content.
#[derive(Clone)]
pub struct PackageCache {
    inner: Arc<Mutex<PackageCacheInner>>,
}

/// Provides a unique identifier for packages in the cache.
/// TODO: This could not be unique over multiple subdir. How to handle?
/// TODO: Wouldn't it be better to cache based on hashes?
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct CacheKey {
    name: String,
    version: String,
    build_string: String,
}

impl From<ArchiveIdentifier> for CacheKey {
    fn from(pkg: ArchiveIdentifier) -> Self {
        CacheKey {
            name: pkg.name,
            version: pkg.version,
            build_string: pkg.build_string,
        }
    }
}

impl From<&PackageRecord> for CacheKey {
    fn from(record: &PackageRecord) -> Self {
        Self {
            name: record.name.as_normalized().to_string(),
            version: record.version.to_string(),
            build_string: record.build.to_string(),
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
    packages: FxHashMap<CacheKey, Arc<Mutex<Package>>>,
}

#[derive(Default)]
struct Package {
    path: Option<PathBuf>,
    inflight: Option<broadcast::Sender<Result<PathBuf, PackageCacheError>>>,
}

/// An error that might be returned from one of the caching function of the [`PackageCache`].
#[derive(Debug, Clone, thiserror::Error)]
pub enum PackageCacheError {
    /// An error occurred while fetching the package.
    #[error(transparent)]
    FetchError(#[from] Arc<dyn std::error::Error + Send + Sync + 'static>),
}

impl PackageCache {
    /// Constructs a new [`PackageCache`] located at the specified path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PackageCacheInner {
                path: path.into(),
                packages: Default::default(),
            })),
        }
    }

    /// Returns the directory that contains the specified package.
    ///
    /// If the package was previously successfully fetched and stored in the cache the directory
    /// containing the data is returned immediately. If the package was not previously fetch the
    /// filesystem is checked to see if a directory with valid package content exists. Otherwise,
    /// the user provided `fetch` function is called to populate the cache.
    ///
    /// If the package is already being fetched by another task/thread the request is coalesced. No
    /// duplicate fetch is performed.
    pub async fn get_or_fetch<F, Fut, E>(
        &self,
        pkg: impl Into<CacheKey>,
        fetch: F,
    ) -> Result<PathBuf, PackageCacheError>
    where
        F: (FnOnce(PathBuf) -> Fut) + Send + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let cache_key = pkg.into();

        // Get the package entry
        let (package, pkg_cache_dir) = {
            let mut inner = self.inner.lock().unwrap();
            let destination = inner.path.join(cache_key.to_string());
            let package = inner.packages.entry(cache_key).or_default().clone();
            (package, destination)
        };

        let mut rx = {
            // Only sync code in this block
            let mut inner = package.lock().unwrap();

            // If there exists an existing value in our cache, we can return that.
            if let Some(path) = inner.path.as_ref() {
                return Ok(path.clone());
            }

            // Is there an in-flight requests for the package?
            if let Some(inflight) = inner.inflight.as_ref() {
                inflight.subscribe()
            } else {
                // There is no in-flight requests so we start one!
                let (tx, rx) = broadcast::channel(1);
                inner.inflight = Some(tx.clone());

                let package = package.clone();
                tokio::spawn(async move {
                    let result = validate_or_fetch_to_cache(pkg_cache_dir.clone(), fetch)
                        .instrument(
                            tracing::debug_span!("validating", path = %pkg_cache_dir.display()),
                        )
                        .await;

                    {
                        // only sync code in this block
                        let mut package = package.lock().unwrap();
                        package.inflight = None;

                        match result {
                            Ok(_) => {
                                package.path.replace(pkg_cache_dir.clone());
                                let _ = tx.send(Ok(pkg_cache_dir));
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e));
                            }
                        }
                    }
                });

                rx
            }
        };

        rx.recv().await.expect("in-flight request has died")
    }

    /// Returns the directory that contains the specified package.
    ///
    /// This is a convenience wrapper around `get_or_fetch` which fetches the package from the given
    /// URL if the package could not be found in the cache.
    pub async fn get_or_fetch_from_url(
        &self,
        pkg: impl Into<CacheKey>,
        url: Url,
        client: AuthenticatedClient,
    ) -> Result<PathBuf, PackageCacheError> {
        self.get_or_fetch_from_url_with_retry(pkg, url, client, DoNotRetryPolicy)
            .await
    }

    /// Returns the directory that contains the specified package.
    ///
    /// This is a convenience wrapper around `get_or_fetch` which fetches the package from the given
    /// URL if the package could not be found in the cache.
    pub async fn get_or_fetch_from_url_with_retry(
        &self,
        pkg: impl Into<CacheKey>,
        url: Url,
        client: AuthenticatedClient,
        retry_policy: impl RetryPolicy + Send + 'static,
    ) -> Result<PathBuf, PackageCacheError> {
        self.get_or_fetch(pkg, move |destination| async move {
            let mut current_try = 0;
            loop {
                current_try += 1;
                tracing::debug!("downloading {} to {}", &url, destination.display());
                let result = rattler_package_streaming::reqwest::tokio::extract(
                    client.clone(),
                    url.clone(),
                    &destination,
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
                        .map(|status| status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::REQUEST_TIMEOUT)
                        .unwrap_or(false)
                ) {
                    return Err(err);
                }

                // Determine whether or not to retry based on the retry policy
                let execute_after = match retry_policy.should_retry(current_try) {
                    RetryDecision::Retry { execute_after } => execute_after,
                    RetryDecision::DoNotRetry => return Err(err),
                };
                let duration = (execute_after - Utc::now()).to_std().expect("the retry duration is out of range");

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
        })
        .await
    }
}

/// Validates that the package that is currently stored is a valid package and otherwise calls the
/// `fetch` method to populate the cache.
async fn validate_or_fetch_to_cache<F, Fut, E>(
    path: PathBuf,
    fetch: F,
) -> Result<(), PackageCacheError>
where
    F: FnOnce(PathBuf) -> Fut + Send,
    Fut: Future<Output = Result<(), E>> + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    // If the directory already exists validate the contents of the package
    if path.is_dir() {
        let path_inner = path.clone();
        match tokio::task::spawn_blocking(move || validate_package_directory(&path_inner)).await {
            Ok(Ok(_)) => {
                tracing::debug!("validation succeeded");
                return Ok(());
            }
            Ok(Err(e)) => {
                tracing::warn!("validation failed: {e}",);
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

    // Otherwise, defer to populate method to fill our cache.
    fetch(path)
        .await
        .map_err(|e| PackageCacheError::FetchError(Arc::new(e)))
}

#[cfg(test)]
mod test {
    use super::PackageCache;
    use crate::{get_test_data_dir, validation::validate_package_directory};
    use assert_matches::assert_matches;
    use axum::{
        extract::State,
        http::{Request, StatusCode},
        middleware,
        middleware::Next,
        response::Response,
        routing::get_service,
        Router,
    };
    use rattler_conda_types::package::{ArchiveIdentifier, PackageFile, PathsJson};
    use rattler_networking::{
        retry_policies::{DoNotRetryPolicy, ExponentialBackoffBuilder},
        AuthenticatedClient,
    };
    use std::{fs::File, net::SocketAddr, path::Path, sync::Arc};
    use tempfile::tempdir;
    use tokio::sync::Mutex;
    use tower_http::services::ServeDir;
    use url::Url;

    #[tokio::test]
    pub async fn test_package_cache() {
        let tar_archive_path =
            get_test_data_dir().join("ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2");

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
        let package_dir = cache
            .get_or_fetch(
                ArchiveIdentifier::try_from_path(&tar_archive_path).unwrap(),
                move |destination| async move {
                    rattler_package_streaming::tokio::fs::extract(&tar_archive_path, &destination)
                        .await
                        .map(|_| ())
                },
            )
            .await
            .unwrap();

        // Validate the contents of the package
        let (_, current_paths) = validate_package_directory(&package_dir).unwrap();

        // Make sure that the paths are the same as what we would expect from the original tar
        // archive.
        assert_eq!(current_paths, paths);
    }

    /// A helper middleware function that fails the first two requests.
    async fn fail_the_first_two_requests<B>(
        State(count): State<Arc<Mutex<i32>>>,
        req: Request<B>,
        next: Next<B>,
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

    #[tokio::test]
    pub async fn test_flaky_package_cache() {
        let static_dir = get_test_data_dir();

        // Construct a service that serves raw files from the test directory
        let service = get_service(ServeDir::new(static_dir));

        // Construct a router that returns data from the static dir but fails the first try.
        let request_count = Arc::new(Mutex::new(0));
        let router =
            Router::new()
                .route_service("/*key", service)
                .layer(middleware::from_fn_with_state(
                    request_count.clone(),
                    fail_the_first_two_requests,
                ));

        // Construct the server that will listen on localhost but with a *random port*. The random
        // port is very important because it enables creating multiple instances at the same time.
        // We need this to be able to run tests in parallel.
        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let server = axum::Server::bind(&addr).serve(router.into_make_service());

        // Get the address of the server so we can bind to it at a later stage.
        let addr = server.local_addr();

        // Spawn the server.
        tokio::spawn(server);

        let packages_dir = tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        let archive_name = "ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2";
        let server_url = Url::parse(&format!("http://localhost:{}", addr.port())).unwrap();

        // Do the first request without
        let result = cache
            .get_or_fetch_from_url_with_retry(
                ArchiveIdentifier::try_from_filename(archive_name).unwrap(),
                server_url.join(archive_name).unwrap(),
                AuthenticatedClient::default(),
                DoNotRetryPolicy,
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
                AuthenticatedClient::default(),
                ExponentialBackoffBuilder::default().build_with_max_retries(3),
            )
            .await;

        assert!(result.is_ok());
        {
            let request_count_lock = request_count.lock().await;
            assert_eq!(*request_count_lock, 3, "Expected there to be 3 requests");
        }
    }
}
