//! This module provides functionality to cache extracted Conda packages. See
//! [`RunExportsCache`].

use std::{
    fmt::Debug,
    future::Future,
    io::Seek,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use dashmap::DashMap;
use download::DownloadError;
use fs_err::tokio as tokio_fs;
use parking_lot::Mutex;
use rattler_conda_types::package::{PackageFile, RunExportsJson};
use rattler_networking::retry_policies::{DoNotRetryPolicy, RetryDecision, RetryPolicy};
use rattler_package_streaming::{DownloadReporter, ExtractError};
use tempfile::{NamedTempFile, PersistError};
use tracing::instrument;
use url::Url;

mod cache_key;
mod download;

pub use cache_key::{CacheKey, CacheKeyError};

use crate::package_cache::CacheReporter;

/// A [`RunExportsCache`] manages a cache of `run_exports.json`
///
/// The store does not provide an implementation to get the data into the store.
/// Instead, this is left up to the user when the `run_exports.json` is requested. If the
/// `run_exports.json` is found in the cache it is returned immediately. However, if the
/// cache is missing a user defined function is called to populate the cache. This
/// separates the corners between caching and fetching of the content.
#[derive(Clone)]
pub struct RunExportsCache {
    inner: Arc<RunExportsCacheInner>,
}

/// A cache entry that contains the path to the package and the `run_exports.json`
#[derive(Clone, Debug)]
pub struct CacheEntry {
    /// The `run_exports.json` of the package.
    pub(crate) run_exports: Option<RunExportsJson>,
    /// The path to the file on disk.
    pub(crate) path: PathBuf,
}

impl CacheEntry {
    /// Create a new cache entry.
    pub(crate) fn new(run_exports: Option<RunExportsJson>, path: PathBuf) -> Self {
        Self { run_exports, path }
    }

    /// Returns the `run_exports.json` of the package.
    pub fn run_exports(&self) -> Option<RunExportsJson> {
        self.run_exports.clone()
    }

    /// Returns the path to the file on disk.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Default)]
struct RunExportsCacheInner {
    path: PathBuf,
    run_exports: DashMap<BucketKey, Arc<tokio::sync::Mutex<Option<CacheEntry>>>>,
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

impl RunExportsCache {
    /// Constructs a new [`RunExportsCache`] located at the specified path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(RunExportsCacheInner {
                path: path.into(),
                run_exports: DashMap::default(),
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
        cache_key: &CacheKey,
        fetch: F,
    ) -> Result<CacheEntry, RunExportsCacheError>
    where
        F: (Fn() -> Fut) + Send + 'static,
        Fut: Future<Output = Result<Option<NamedTempFile>, E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let cache_path = self.inner.path.join(cache_key.to_string());
        let cache_entry = self
            .inner
            .run_exports
            .entry(cache_key.clone().into())
            .or_default()
            .clone();

        // Acquire the entry. From this point on we can be sure that only one task is
        // accessing the cache entry.
        let mut entry = cache_entry.lock().await;

        // Check if the cache entry is already stored in the cache.
        if let Some(run_exports) = entry.as_ref() {
            return Ok(run_exports.clone());
        }

        // Otherwise, defer to populate method to fill our cache.
        let run_exports_file = fetch()
            .await
            .map_err(|e| RunExportsCacheError::Fetch(Arc::new(e)))?;

        if let Some(parent_dir) = cache_path.parent() {
            if !parent_dir.exists() {
                tokio_fs::create_dir_all(parent_dir).await?;
            }
        }

        let run_exports = if let Some(file) = run_exports_file {
            file.persist(&cache_path)?;

            let run_exports_str = tokio_fs::read_to_string(&cache_path).await?;
            Some(RunExportsJson::from_str(&run_exports_str)?)
        } else {
            None
        };

        let cache_entry = CacheEntry::new(run_exports, cache_path);

        entry.replace(cache_entry.clone());

        Ok(cache_entry)
    }

    /// Returns the directory that contains the specified package.
    ///
    /// This is a convenience wrapper around `get_or_fetch` which fetches the
    /// package from the given URL if the package could not be found in the
    /// cache.
    pub async fn get_or_fetch_from_url(
        &self,
        cache_key: &CacheKey,
        url: Url,
        client: reqwest_middleware::ClientWithMiddleware,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<CacheEntry, RunExportsCacheError> {
        self.get_or_fetch_from_url_with_retry(cache_key, url, client, DoNotRetryPolicy, reporter)
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
        cache_key: &CacheKey,
        url: Url,
        client: reqwest_middleware::ClientWithMiddleware,
        retry_policy: impl RetryPolicy + Send + 'static + Clone,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<CacheEntry, RunExportsCacheError> {
        let request_start = SystemTime::now();
        // Convert into cache key
        let download_reporter = reporter.clone();

        let extension = cache_key.extension.clone();
        // Get or fetch the package, using the specified fetch function
        self.get_or_fetch(cache_key, move || {

            #[derive(Debug, thiserror::Error)]
            enum FetchError{
                #[error(transparent)]
                Download(#[from] DownloadError),

                #[error(transparent)]
                Extract(#[from] ExtractError),

                #[error(transparent)]
                Io(#[from] std::io::Error),

            }

            let url = url.clone();
            let client = client.clone();
            let retry_policy = retry_policy.clone();
            let download_reporter = download_reporter.clone();
            let extension = extension.clone();

            async move {
                let mut current_try = 0;
                // Retry until the retry policy says to stop
                loop {
                    current_try += 1;
                    tracing::debug!("downloading {}", &url);
                    // Extract the package

                    let temp_file = if url.scheme() == "file" {
                        let path = url.to_file_path().map_err(|_err| FetchError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid file path")))?;
                        let temp_file = NamedTempFile::with_suffix(&extension)?;
                        tokio_fs::copy(path, temp_file.path()).await?;
                        Ok(temp_file)
                    } else {
                        crate::run_exports_cache::download::download(
                            client.clone(),
                            url.clone(),
                            &extension,
                            download_reporter.clone().map(|reporter| Arc::new(PassthroughReporter {
                                reporter,
                                index: Mutex::new(None),
                            }) as Arc::<dyn DownloadReporter>),
                        )
                            .await
                    };

                    // Extract any potential error
                    let err = match temp_file {
                        Ok(result) => {
                            let output_temp_file = NamedTempFile::new()?;
                            // Clone the file handler to be able to pass it to the blocking task
                            let mut file_handler = output_temp_file.as_file().try_clone()?;
                            // now extract run_exports.json from the archive without unpacking
                            let result = simple_spawn_blocking::tokio::run_blocking_task(move || {
                                rattler_package_streaming::seek::extract_package_file::<RunExportsJson>(result.as_file(), result.path(), &mut file_handler)?;
                                file_handler.rewind()?;
                                Ok(())
                            }).await;

                            match result {
                                Ok(()) => {
                                    return Ok(Some(output_temp_file));
                                },
                                Err(err) => {
                                    if matches!(err, ExtractError::MissingComponent) {
                                        return Ok(None);
                                    }
                                    return Err(FetchError::Extract(err));

                                }
                            }
                        },
                        Err(err) => FetchError::Download(err),
                    };

                    // Only retry on io errors. We assume that the user has
                    // middleware installed that handles connection retries.
                    if !matches!(&err, FetchError::Download(_)) {
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
/// [`RunExportsCache`].
#[derive(Debug, thiserror::Error)]
pub enum RunExportsCacheError {
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

    /// An error occured when extracting `run_exports` from archive
    #[error(transparent)]
    Extract(#[from] ExtractError),

    /// An error occured when serializing `run_exports`
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),

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

    use crate::run_exports_cache::CacheKey;

    use super::RunExportsCache;

    #[tokio::test]
    pub async fn test_run_exports_cache_when_empty() {
        // This archive does not contain a run_exports.json
        // so we expect the cache to return None
        let package_url = Url::parse("https://conda.anaconda.org/robostack/linux-64/ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2").unwrap();

        let cache_dir = tempdir().unwrap().into_path();

        let cache = RunExportsCache::new(&cache_dir);

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

        let cache_key = CacheKey::create(
            &pkg_record,
            "ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2",
        )
        .unwrap();

        // Get the package to the cache
        let cached_run_exports = cache
            .get_or_fetch_from_url(
                &cache_key,
                package_url.clone(),
                ClientWithMiddleware::from(Client::new()),
                None,
            )
            .await
            .unwrap();

        assert!(cached_run_exports.run_exports.is_none());
    }

    #[tokio::test]
    pub async fn test_run_exports_cache_when_present() {
        // This archive contains a run_exports.json
        // so we expect the cache to return it
        let package_url =
            Url::parse("https://repo.prefix.dev/conda-forge/linux-64/zlib-1.3.1-hb9d3cd8_2.conda")
                .unwrap();

        let cache_dir = tempdir().unwrap().into_path();

        let cache = RunExportsCache::new(&cache_dir);

        let pkg_record = PackageRecord::new(
            PackageName::from_str("zlib").unwrap(),
            Version::from_str("1.3.1").unwrap(),
            "hb9d3cd8_2".to_string(),
        );

        let cache_key = CacheKey::create(&pkg_record, "zlib-1.3.1-hb9d3cd8_2.conda").unwrap();

        // Get the package to the cache
        let cached_run_exports = cache
            .get_or_fetch_from_url(
                &cache_key,
                package_url.clone(),
                ClientWithMiddleware::from(Client::new()),
                None,
            )
            .await
            .unwrap();

        assert!(cached_run_exports.run_exports.is_some());
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
        let cache = RunExportsCache::new(packages_dir.path());

        let server_url = Url::parse(&format!("http://localhost:{}", addr.port())).unwrap();

        let client = ClientBuilder::new(Client::default()).build();

        let cache_key = CacheKey::create(package_record, archive_name).unwrap();

        // Do the first request without
        let result = cache
            .get_or_fetch_from_url_with_retry(
                &cache_key,
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
                &cache_key,
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
        let cache = RunExportsCache::new(packages_dir.path());

        let cache_key = CacheKey::create(
            &pkg_record,
            "ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2",
        )
        .unwrap();

        // Get the package to the cache
        let first_cache_path = cache
            .get_or_fetch_from_url(
                &cache_key,
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

        let cache_key = CacheKey::create(
            &pkg_record,
            "ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2",
        )
        .unwrap();

        // Get the package again
        // and verify that the package was replaced
        let second_package_cache = cache
            .get_or_fetch_from_url(
                &cache_key,
                package_url.clone(),
                ClientWithMiddleware::from(Client::new()),
                None,
            )
            .await
            .unwrap();

        assert_ne!(first_cache_path.path(), second_package_cache.path());
    }

    #[tokio::test]
    // Test caching a run exports file by file:// URL
    pub async fn test_file_path_archive() {
        let package_path = tools::download_and_cache_file_async(
            "https://repo.prefix.dev/conda-forge/linux-64/zlib-1.3.1-hb9d3cd8_2.conda"
                .parse()
                .unwrap(),
            "5d7c0e5f0005f74112a34a7425179f4eb6e73c92f5d109e6af4ddeca407c92ab",
        )
        .await
        .unwrap();

        let cache_dir = tempdir().unwrap().into_path();

        let cache = RunExportsCache::new(&cache_dir);

        let pkg_record = PackageRecord::new(
            PackageName::from_str("zlib").unwrap(),
            Version::from_str("1.3.1").unwrap(),
            "hb9d3cd8_2".to_string(),
        );

        let cache_key = CacheKey::create(&pkg_record, "zlib-1.3.1-hb9d3cd8_2.conda").unwrap();

        // Get the package to the cache
        let cached_run_exports = cache
            .get_or_fetch_from_url(
                &cache_key,
                Url::from_file_path(package_path).expect("we have a valid file path"),
                ClientWithMiddleware::from(Client::new()),
                None,
            )
            .await
            .unwrap();

        assert!(cached_run_exports.run_exports.is_some());
    }
}
