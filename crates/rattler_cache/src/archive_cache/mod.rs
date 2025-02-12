//! This module provides functionality to cache extracted Conda packages. See
//! [`ArchiveCache`].

use std::{
    fmt::Debug,
    future::Future,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use dashmap::DashMap;
use download::DownloadError;
use fs_err::tokio as tokio_fs;
use parking_lot::Mutex;
use rattler_networking::retry_policies::{DoNotRetryPolicy, RetryDecision, RetryPolicy};
use rattler_package_streaming::DownloadReporter;
use tempfile::{NamedTempFile, PersistError};
use tracing::instrument;
use url::Url;

mod cache_key;
mod download;

use cache_key::CacheKey;

use crate::package_cache::CacheReporter;

/// A [`ArchiveCache`] manages a cache of Conda packages on disk.
///
/// The store does not provide an implementation to get the data into the store.
/// Instead, this is left up to the user when the package is requested. If the
/// package is found in the cache it is returned immediately. However, if the
/// cache is missing a user defined function is called to populate the cache. This
/// separates the corners between caching and fetching of the content.
#[derive(Clone)]
pub struct ArchiveCache {
    inner: Arc<ArchiveCacheInner>,
}

#[derive(Default)]
struct ArchiveCacheInner {
    path: PathBuf,
    packages: DashMap<BucketKey, Arc<tokio::sync::Mutex<()>>>,
}

/// A key that defines the actual location of the package in the cache.
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct BucketKey {
    name: String,
    version: String,
    build_string: String,
    sha256_string: String,
}

impl From<CacheKey> for BucketKey {
    fn from(key: CacheKey) -> Self {
        Self {
            name: key.name.clone(),
            version: key.version.clone(),
            build_string: key.build_string.clone(),
            sha256_string: key.sha256_str(),
        }
    }
}

impl ArchiveCache {
    /// Constructs a new [`ArchiveCache`] located at the specified path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(ArchiveCacheInner {
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
    ) -> Result<PathBuf, ArchiveCacheError>
    where
        F: (Fn() -> Fut) + Send + 'static,
        Fut: Future<Output = Result<NamedTempFile, E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let cache_key = pkg.into();
        let cache_path = self.inner.path.join(cache_key.to_string());
        let cache_entry = self
            .inner
            .packages
            .entry(cache_key.clone().into())
            .or_default()
            .clone();

        // Acquire the entry. From this point on we can be sure that only one task is
        // accessing the cache entry.
        let _ = cache_entry.lock().await;

        // Check if the cache entry is already stored in the cache.
        if cache_path.exists() {
            return Ok(cache_path);
        }

        // Otherwise, defer to populate method to fill our cache.
        let temp_file = fetch()
            .await
            .map_err(|e| ArchiveCacheError::Fetch(Arc::new(e)))?;

        if let Some(parent_dir) = cache_path.parent() {
            if !parent_dir.exists() {
                tokio_fs::create_dir_all(parent_dir).await?;
            }
        }

        temp_file.persist(&cache_path)?;

        Ok(cache_path)
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
    ) -> Result<PathBuf, ArchiveCacheError> {
        self.get_or_fetch_from_url_with_retry(pkg, url, client, DoNotRetryPolicy, reporter)
            .await
    }

    /// Returns the directory that contains the specified package.
    ///
    /// This is a convenience wrapper around `get_or_fetch` which fetches the
    /// package from the given URL if the package could not be found in the
    /// cache.
    ///
    /// This function assumes that the `client` is already configured with a
    /// retry middleware that will retry any request that fails. This function
    /// uses the passed in `retry_policy` if, after the request has been sent
    /// and the response is successful, streaming of the package data fails
    /// and the whole request must be retried.
    #[instrument(skip_all, fields(url=%url))]
    pub async fn get_or_fetch_from_url_with_retry(
        &self,
        pkg: impl Into<CacheKey>,
        url: Url,
        client: reqwest_middleware::ClientWithMiddleware,
        retry_policy: impl RetryPolicy + Send + 'static + Clone,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<PathBuf, ArchiveCacheError> {
        let request_start = SystemTime::now();
        // Convert into cache key
        let cache_key = pkg.into();
        let download_reporter = reporter.clone();
        // Get or fetch the package, using the specified fetch function
        self.get_or_fetch(cache_key, move || {
            let url = url.clone();
            let client = client.clone();
            let retry_policy = retry_policy.clone();
            let download_reporter = download_reporter.clone();
            async move {
                let mut current_try = 0;
                // Retry until the retry policy says to stop
                loop {
                    current_try += 1;
                    tracing::debug!("downloading {}", &url);
                    // Extract the package
                    let result = crate::archive_cache::download::download(
                        client.clone(),
                        url.clone(),
                        // &temp_file,
                        download_reporter.clone().map(|reporter| Arc::new(PassthroughReporter {
                            reporter,
                            index: Mutex::new(None),
                        }) as Arc::<dyn DownloadReporter>),
                    )
                        .await;

                    // Extract any potential error
                    let err = match result {
                        Ok(result) => return Ok(result),
                        Err(err) => err,
                    };

                    // Only retry on io errors. We assume that the user has
                    // middleware installed that handles connection retries.
                    if !matches!(&err,DownloadError::Io(_)) {
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
                        "failed to download and extract {} {}. Retry #{}, Sleeping {:?} until the next attempt...",
                        &url,
                        // destination.display(),
                        err,
                        current_try,
                        duration
                    );
                    tokio::time::sleep(duration).await;
                }
            }
        })
            .await
    }
}

/// An error that might be returned from one of the caching function of the
/// [`ArchiveCache`].
#[derive(Debug, thiserror::Error)]
pub enum ArchiveCacheError {
    /// An error occurred while fetching the package.
    #[error(transparent)]
    Fetch(#[from] Arc<dyn std::error::Error + Send + Sync + 'static>),

    /// A locking error occurred
    #[error("{0}")]
    Lock(String, #[source] std::io::Error),

    /// An IO error occurred
    #[error("{0}")]
    Io(#[from] std::io::Error),

    /// An error occurred while persisting the temp file
    #[error("{0}")]
    Persist(#[from] PersistError),

    /// The operation was cancelled
    #[error("operation was cancelled")]
    Cancelled,
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
    use std::{future::IntoFuture, net::SocketAddr, str::FromStr, sync::Arc};

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

    use rattler_conda_types::{PackageName, PackageRecord, Version};
    use rattler_digest::{parse_digest_from_hex, Sha256};
    use rattler_networking::retry_policies::{DoNotRetryPolicy, ExponentialBackoffBuilder};
    use reqwest::Client;
    use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
    use reqwest_retry::RetryTransientMiddleware;
    use tempfile::tempdir;
    use tokio::sync::Mutex;

    use url::Url;

    use super::ArchiveCache;

    #[tokio::test]
    pub async fn test_package_cache() {
        let package_url = Url::parse("https://conda.anaconda.org/robostack/linux-64/ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2").unwrap();

        let cache_dir = tempdir().unwrap().into_path();

        let cache = ArchiveCache::new(&cache_dir);

        let mut pkg_record = PackageRecord::new(
            PackageName::from_str("ros-noetic-rosbridge-suite").unwrap(),
            Version::from_str("0.11.14").unwrap(),
            "py39h6fdeb60_14".to_string(),
        );
        pkg_record.sha256 = Some(
            parse_digest_from_hex::<Sha256>(
                "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8",
            )
            .unwrap(),
        );

        // Get the package to the cache
        let cache_path = cache
            .get_or_fetch_from_url(
                &pkg_record,
                package_url.clone(),
                ClientWithMiddleware::from(Client::new()),
                None,
            )
            .await
            .unwrap();

        assert!(cache_path.exists());
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

    enum Middleware {
        FailTheFirstTwoRequests,
    }

    async fn redirect_to_prefix(
        axum::extract::Path((channel, subdir, file)): axum::extract::Path<(String, String, String)>,
    ) -> Redirect {
        Redirect::permanent(&format!("https://prefix.dev/{channel}/{subdir}/{file}"))
    }

    async fn test_flaky_package_cache(
        archive_name: &str,
        package_record: &PackageRecord,
        middleware: Middleware,
    ) {
        // Construct a service that serves raw files from the test directory
        // build our application with a route
        let router = Router::new()
            // `GET /` goes to `root`
            .route("/{channel}/{subdir}/{file}", get(redirect_to_prefix));

        // Construct a router that returns data from the static dir but fails the first
        // try.
        let request_count = Arc::new(Mutex::new(0));

        let router = match middleware {
            Middleware::FailTheFirstTwoRequests => router.layer(middleware::from_fn_with_state(
                request_count.clone(),
                fail_the_first_two_requests,
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
        let cache = ArchiveCache::new(packages_dir.path());

        let server_url = Url::parse(&format!("http://localhost:{}", addr.port())).unwrap();

        let client = ClientBuilder::new(Client::default()).build();

        // Do the first request without
        let result = cache
            .get_or_fetch_from_url_with_retry(
                package_record,
                server_url.join(archive_name).unwrap(),
                client.clone(),
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

        let retry_policy = ExponentialBackoffBuilder::default().build_with_max_retries(3);
        let client = ClientBuilder::from_client(client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        // The second one should fail after the 2nd try
        let result = cache
            .get_or_fetch_from_url_with_retry(
                package_record,
                server_url.join(archive_name).unwrap(),
                client,
                retry_policy,
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

        let tar_record = PackageRecord::new(
            PackageName::from_str("conda").unwrap(),
            Version::from_str("22.9.0").unwrap(),
            "py310h5588dad_2".to_string(),
        );

        let conda_record = PackageRecord::new(
            PackageName::from_str("conda").unwrap(),
            Version::from_str("22.11.1").unwrap(),
            "py38haa244fe_1".to_string(),
        );

        test_flaky_package_cache(tar_bz2, &tar_record, Middleware::FailTheFirstTwoRequests).await;
        test_flaky_package_cache(conda, &conda_record, Middleware::FailTheFirstTwoRequests).await;
    }

    #[tokio::test]
    // Test if packages with different sha's are replaced even though they share the
    // same BucketKey.
    pub async fn test_package_cache_key_with_sha() {
        let package_url = Url::parse("https://conda.anaconda.org/robostack/linux-64/ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2").unwrap();

        let mut pkg_record = PackageRecord::new(
            PackageName::from_str("ros-noetic-rosbridge-suite").unwrap(),
            Version::from_str("0.11.14").unwrap(),
            "py39h6fdeb60_14".to_string(),
        );
        pkg_record.sha256 = Some(
            parse_digest_from_hex::<Sha256>(
                "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8",
            )
            .unwrap(),
        );

        // Create a temporary directory to store the packages
        let packages_dir = tempdir().unwrap();
        let cache = ArchiveCache::new(packages_dir.path());

        // Get the package to the cache
        let first_cache_path = cache
            .get_or_fetch_from_url(
                &pkg_record,
                package_url.clone(),
                ClientWithMiddleware::from(Client::new()),
                None,
            )
            .await
            .unwrap();

        // Change the sha256 of the package
        // And expect the package to be replaced
        let new_sha = parse_digest_from_hex::<Sha256>(
            "5dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc9",
        )
        .unwrap();
        pkg_record.sha256 = Some(new_sha);

        // Get the package again
        // and verify that the package was replaced
        let second_package_cache = cache
            .get_or_fetch_from_url(
                &pkg_record,
                package_url.clone(),
                ClientWithMiddleware::from(Client::new()),
                None,
            )
            .await
            .unwrap();

        assert_ne!(first_cache_path, second_package_cache);
    }
}
