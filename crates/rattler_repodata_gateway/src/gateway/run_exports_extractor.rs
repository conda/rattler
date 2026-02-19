use std::{
    io::BufReader,
    path::{Path, PathBuf},
    sync::Arc,
};

use bytes::Buf;
use coalesced_map::CoalescedMap;
use http::StatusCode;
use rattler_conda_types::{package::RunExportsJson, RepoDataRecord, SubdirRunExportsJson};
use rattler_networking::LazyClient;
use reqwest_middleware::ClientWithMiddleware;
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::instrument;
use url::Url;

use crate::reporter::{DownloadReporter, ResponseReporterExt};

/// Type used for in-memory caching of `SubdirRunExportsJson`.
pub(crate) type SubdirRunExportsCache = CoalescedMap<Url, Option<Arc<SubdirRunExportsJson>>>;

/// A trait that enables being notified of download progress for run exports.
pub trait RunExportsReporter: Send + Sync {
    /// Returns a reporter that can be used to report download progress.
    fn download_reporter(&self) -> Option<&dyn DownloadReporter>;

    /// Called to create a new reporter than can be used to report progress of a
    /// package download.
    #[cfg(not(target_arch = "wasm32"))]
    fn create_package_download_reporter(
        &self,
        _repo_data_record: &RepoDataRecord,
    ) -> Option<Box<dyn rattler_cache::package_cache::CacheReporter>> {
        None
    }
}

/// An object that can help extract run export information from a package.
///
/// This object can be configured with multiple sources and it will do its best
/// to find the run exports as fast as possible using the available resources.
#[derive(Default)]
pub struct RunExportExtractor {
    max_concurrent_requests: Option<Arc<Semaphore>>,
    client: Option<LazyClient>,
    subdir_run_exports_cache: Arc<SubdirRunExportsCache>,

    #[cfg(not(target_arch = "wasm32"))]
    package_cache: Option<rattler_cache::package_cache::PackageCache>,
}

#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum RunExportExtractorError {
    #[cfg(not(target_arch = "wasm32"))]
    #[error(transparent)]
    PackageCache(#[from] rattler_cache::package_cache::PackageCacheError),

    #[error("failed to request run exports from {0}")]
    Request(Url, #[source] reqwest_middleware::Error),

    #[error("failed to decode run exports from {0}")]
    DecodeRunExports(String, #[source] std::io::Error),

    #[error("failed download bytes from {0}")]
    TransportError(Url, #[source] reqwest::Error),

    #[error("the operation was cancelled")]
    Cancelled,
}

impl RunExportExtractor {
    /// Sets the maximum number of concurrent requests that the extractor can
    /// make.
    pub fn with_opt_max_concurrent_requests(
        self,
        max_concurrent_requests: Option<Arc<Semaphore>>,
    ) -> Self {
        Self {
            max_concurrent_requests,
            ..self
        }
    }

    /// Set the package cache that the extractor can use as well as a reporter
    /// to allow progress reporting.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_package_cache(
        self,
        package_cache: rattler_cache::package_cache::PackageCache,
    ) -> Self {
        Self {
            package_cache: Some(package_cache),
            ..self
        }
    }

    /// Sets the download client that the extractor can use.
    pub fn with_client(self, client: LazyClient) -> Self {
        Self {
            client: Some(client),
            ..self
        }
    }

    /// Sets the download client that the extractor can use.
    pub fn with_global_run_exports_cache(
        self,
        global_run_exports_cache: Arc<SubdirRunExportsCache>,
    ) -> Self {
        Self {
            subdir_run_exports_cache: global_run_exports_cache,
            ..self
        }
    }

    /// Extract the run exports from a package by first checking the
    /// `run_exports.json` file in the channel subdirectory, and if that fails,
    /// it will download the package to the cache and read the
    /// `run_exports.json` file from there.
    #[instrument(skip_all, fields(record = %record.url.as_str()))]
    pub async fn extract(
        mut self,
        record: &RepoDataRecord,
        progress_reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        // If this is local package, try to read it directly from the package file
        // itself.
        if let Some(package_file) = Self::path_to_package(record) {
            return self.read_from_package_file(&package_file).await;
        }

        // Try to fetch the `run_exports.json` from channel
        if let Some(subdir_run_exports) = self
            .fetch_subdir_run_exports(&record.platform_url(), progress_reporter.clone())
            .await
        {
            // If the package is found in the run_exports.json, return its run_exports.
            // Otherwise, fall through to extracting from the package cache (the
            // run_exports.json might be out of sync with the actual packages).
            if let Some(run_exports) = subdir_run_exports.get(record).cloned() {
                return Ok(Some(run_exports));
            }
        }

        // Otherwise, fall back to extracting from the package cache.
        if let Some(package_cache_run_exports) = self
            .extract_into_package_cache(record, progress_reporter)
            .await?
        {
            return Ok(Some(package_cache_run_exports));
        }

        Ok(None)
    }

    /// Returns the path to the package file if the URL is a file URL.
    fn path_to_package(_record: &RepoDataRecord) -> Option<PathBuf> {
        #[cfg(not(target_arch = "wasm32"))]
        return _record.url.to_file_path().ok();
        #[cfg(target_arch = "wasm32")]
        None
    }

    /// Fetch the `run_exports.json.zst` file from the subdirectory URL.
    async fn fetch_subdir_run_exports_zst_json(
        &self,
        subdir_url: &Url,
        client: &ClientWithMiddleware,
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<Option<SubdirRunExportsJson>, RunExportExtractorError> {
        let url = subdir_url
            .join("run_exports.json.zst")
            .expect("is a valid url segment");

        let _permit = self.acquire_request_permit().await;

        let reporter = reporter
            .as_deref()
            .and_then(RunExportsReporter::download_reporter)
            .map(|reporter| (reporter, reporter.on_download_start(&url)));
        let _progress_guard = DownloadProgressGuard::new(reporter, url.clone());

        let request = client.get(url.clone());
        let response = request.send().await.and_then(|resp| {
            resp.error_for_status()
                .map_err(reqwest_middleware::Error::Reqwest)
        });
        match response {
            Ok(response) => {
                let bytes_stream = match response.bytes_with_progress(reporter).await {
                    Ok(bytes) => bytes,
                    Err(err) => return Err(RunExportExtractorError::TransportError(url, err)),
                };
                let buf = BufReader::new(bytes_stream.reader());
                let decoded = match zstd::decode_all(buf) {
                    Ok(decoded) => decoded,
                    Err(err) => {
                        return Err(RunExportExtractorError::DecodeRunExports(
                            url.to_string(),
                            err,
                        ))
                    }
                };
                let run_exports = match serde_json::from_slice(&decoded) {
                    Ok(run_exports) => Some(run_exports),
                    Err(e) => {
                        return Err(RunExportExtractorError::DecodeRunExports(
                            url.to_string(),
                            e.into(),
                        ))
                    }
                };

                Ok(run_exports)
            }
            Err(err) if err.status() != Some(StatusCode::NOT_FOUND) => {
                Err(RunExportExtractorError::Request(url, err))
            }
            _ => Ok(None),
        }
    }

    /// Fetch the `run_exports.json` file from the subdirectory URL.
    async fn fetch_subdir_run_exports_json(
        &self,
        subdir_url: &Url,
        client: &ClientWithMiddleware,
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<Option<SubdirRunExportsJson>, RunExportExtractorError> {
        let url = subdir_url
            .join("run_exports.json")
            .expect("is a valid url segment");

        let _permit = self.acquire_request_permit().await;

        let request = client.get(url.clone());
        let reporter = reporter
            .as_deref()
            .and_then(RunExportsReporter::download_reporter)
            .map(|reporter| (reporter, reporter.on_download_start(&url)));
        let _progress_guard = DownloadProgressGuard::new(reporter, url.clone());

        let response = request.send().await.and_then(|resp| {
            resp.error_for_status()
                .map_err(reqwest_middleware::Error::Reqwest)
        });
        match response {
            Ok(response) => {
                let bytes_stream = match response.bytes_with_progress(reporter).await {
                    Ok(bytes) => bytes,
                    Err(err) => return Err(RunExportExtractorError::TransportError(url, err)),
                };
                let run_exports = match serde_json::from_slice(&bytes_stream) {
                    Ok(run_exports) => Some(run_exports),
                    Err(e) => {
                        return Err(RunExportExtractorError::DecodeRunExports(
                            url.to_string(),
                            e.into(),
                        ))
                    }
                };
                Ok(run_exports)
            }
            Err(err) if err.status() != Some(StatusCode::NOT_FOUND) => {
                Err(RunExportExtractorError::Request(url, err))
            }
            _ => Ok(None),
        }
    }

    /// Fetch the `run_exports.json` file from the subdirectory URL, either from
    /// the `run_exports.json.zst` file or the `run_exports.json` file.
    async fn fetch_subdir_run_exports(
        &mut self,
        subdir_url: &Url,
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Option<Arc<SubdirRunExportsJson>> {
        let url = subdir_url.clone();
        let client = self.client.clone()?;

        self.subdir_run_exports_cache
            .get_or_try_init(url, || async {
                // Try to fetch the `run_exports.json.zst` file first, and if that
                // fails, fall back to the `run_exports.json` file.
                let mut run_exports = self
                    .fetch_subdir_run_exports_zst_json(
                        subdir_url,
                        client.client(),
                        reporter.clone(),
                    )
                    .await?;
                if run_exports.is_none() {
                    run_exports = self
                        .fetch_subdir_run_exports_json(
                            subdir_url,
                            client.client(),
                            reporter.clone(),
                        )
                        .await?;
                }

                // Package it up in an `Arc` so that it can be shared.
                let run_exports = run_exports.map(Arc::new);

                Ok::<_, RunExportExtractorError>(run_exports)
            })
            .await
            .unwrap_or(None)
    }

    /// Extract the run exports from a package by downloading it to the cache
    /// and then reading the `run_exports.json` file.
    #[cfg(not(target_arch = "wasm32"))]
    async fn extract_into_package_cache(
        self,
        record: &RepoDataRecord,
        progress_reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        use rattler_cache::package_cache::CacheKey;

        let (Some(package_cache), Some(client)) =
            (self.package_cache.as_ref(), self.client.as_ref())
        else {
            return Ok(None);
        };

        let cache_key = CacheKey::from(&record.package_record);

        // Construct a reporter specifically for the run export download
        let reporter = progress_reporter
            .as_deref()
            .and_then(|reporter| reporter.create_package_download_reporter(record))
            .map(Arc::from);

        // Wait for a permit from the semaphore to limit concurrent requests.
        let _permit = self.acquire_request_permit().await;

        match package_cache
            .get_or_fetch_from_url_with_retry(
                cache_key,
                record.url.clone(),
                client.clone(),
                rattler_networking::retry_policies::default_retry_policy(),
                reporter,
            )
            .await
        {
            Ok(package_dir) => Ok(<RunExportsJson as rattler_conda_types::package::PackageFile>::from_package_directory(package_dir.path()).ok()),
            Err(e) => Err(e.into()),
        }
    }

    /// Acquire a permit to limit the number of concurrent requests.
    pub async fn acquire_request_permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        futures::future::OptionFuture::from(
            self.max_concurrent_requests
                .clone()
                .map(Semaphore::acquire_owned),
        )
        .await
        .transpose()
        .expect("semaphore error")
    }

    #[cfg(target_arch = "wasm32")]
    async fn extract_into_package_cache(
        self,
        _record: &RepoDataRecord,
        _progress_reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        Ok(None)
    }

    /// Read the `run_exports.json` file from a package file directly.
    #[cfg(not(target_arch = "wasm32"))]
    async fn read_from_package_file(
        &self,
        path: &Path,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        match rattler_package_streaming::seek::read_package_file(path) {
            Ok(run_exports_json) => Ok(Some(run_exports_json)),
            Err(rattler_package_streaming::ExtractError::MissingComponent) => Ok(None),
            Err(rattler_package_streaming::ExtractError::IoError(err)) => Err(
                RunExportExtractorError::DecodeRunExports(path.display().to_string(), err),
            ),
            Err(err) => Err(RunExportExtractorError::DecodeRunExports(
                path.display().to_string(),
                std::io::Error::other(err),
            )),
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn read_from_package_file(
        &self,
        _path: &Path,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        Ok(None)
    }
}

/// RAII guard to automatically complete download progress reporting when
/// dropped.
struct DownloadProgressGuard<'a> {
    reporter: Option<(&'a dyn DownloadReporter, usize)>,
    url: Url,
}

impl<'a> DownloadProgressGuard<'a> {
    fn new(reporter: Option<(&'a dyn DownloadReporter, usize)>, url: Url) -> Self {
        Self { reporter, url }
    }
}

impl<'a> Drop for DownloadProgressGuard<'a> {
    fn drop(&mut self) {
        if let Some((reporter, index)) = self.reporter {
            reporter.on_download_complete(&self.url, index);
        }
    }
}
