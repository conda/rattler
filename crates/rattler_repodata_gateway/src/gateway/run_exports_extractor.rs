use std::{io::BufReader, sync::Arc};

use bytes::Buf;
use dashmap::{DashMap, Entry};
use futures::future::OptionFuture;
use rattler_cache::package_cache::{CacheKey, CacheReporter, PackageCache, PackageCacheError};
use rattler_conda_types::{
    package::{PackageFile, RunExportsJson},
    RepoDataRecord, SubdirRunExportsJson,
};
use rattler_networking::retry_policies::default_retry_policy;
use reqwest_middleware::ClientWithMiddleware;
use thiserror::Error;
use tokio::sync::{broadcast, Semaphore};
use url::Url;

use super::PendingOrFetched;

/// Type used for in-memory caching of `SubdirRunExportsJson`.
pub(crate) type SubdirRunExportsCache =
    DashMap<Url, PendingOrFetched<Option<Arc<SubdirRunExportsJson>>>>;

pub trait RunExportsReporter: Send + Sync {
    /// Called when a download of a file started.
    ///
    /// Returns an index that can be used to identify the download in subsequent
    /// calls.
    fn on_download_start(&self, _url: &Url) -> usize {
        0
    }

    /// Called when the download of a file makes any progress.
    ///
    /// The `total_bytes` parameter is `None` if the total size of the file is
    /// unknown.
    ///
    /// The `index` parameter is the index returned by `on_download_start`.
    fn on_download_progress(
        &self,
        _url: &Url,
        _index: usize,
        _bytes_downloaded: usize,
        _total_bytes: Option<usize>,
    ) {
    }

    /// Called when the download of a file finished.
    ///
    /// The `index` parameter is the index returned by `on_download_start`.
    fn on_download_complete(&self, _url: &Url, _index: usize) {}

    /// Called to create a new reporter than can be used to report progress of a
    /// package download.
    fn create_package_download_reporter(&self) -> Option<Box<dyn CacheReporter>> {
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
    package_cache: Option<PackageCache>,
    client: Option<ClientWithMiddleware>,
    subdir_run_exports_cache: Arc<SubdirRunExportsCache>,
}

#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum RunExportExtractorError {
    #[error(transparent)]
    PackageCache(#[from] PackageCacheError),

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
    pub fn with_package_cache(self, package_cache: PackageCache) -> Self {
        Self {
            package_cache: Some(package_cache),
            ..self
        }
    }

    /// Sets the download client that the extractor can use.
    pub fn with_client(self, client: ClientWithMiddleware) -> Self {
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

    async fn fetch_subdir_run_exports(
        &mut self,
        subdir_url: &Url,
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Option<Arc<SubdirRunExportsJson>> {
        let sender = match self.subdir_run_exports_cache.entry(subdir_url.clone()) {
            Entry::Vacant(entry) => {
                // Construct a sender so other tasks can subscribe
                let (sender, _) = broadcast::channel(1);
                let sender = Arc::new(sender);

                // Modify the current entry to the pending entry, this is an atomic operation
                // because who holds the entry holds mutable access.
                entry.insert(PendingOrFetched::Pending(Arc::downgrade(&sender)));

                sender
            }
            Entry::Occupied(mut entry) => {
                let records = entry.get();
                match records {
                    PendingOrFetched::Pending(sender) => {
                        let sender = sender.upgrade();

                        if let Some(sender) = sender {
                            // Create a receiver before we drop the entry. While we hold on to
                            // the entry we have exclusive access to it, this means the task
                            // currently fetching the package will not be able to store a value
                            // until we drop the entry.
                            // By creating the receiver here we ensure that we are subscribed
                            // before the other tasks sends a value over the channel.
                            let mut receiver = sender.subscribe();

                            // Explicitly drop the entry, so we don't block any other tasks.
                            drop(entry);

                            // The sender is still active, so we can wait for the records to be
                            // fetched.
                            return receiver.recv().await.unwrap_or_else(|_| None);
                        } else {
                            // Construct a sender so other tasks can subscribe
                            let (sender, _) = broadcast::channel(1);
                            let sender = Arc::new(sender);

                            // Modify the current entry to the pending entry, this is an atomic
                            // operation because who holds the entry holds mutable access.
                            entry.insert(PendingOrFetched::Pending(Arc::downgrade(&sender)));

                            sender
                        }
                    }
                    PendingOrFetched::Fetched(records) => return records.clone(),
                }
            }
        };

        // At this point we have exclusive write access to this specific entry. All
        // other tasks will find a pending entry and will wait for the records
        // to become available.
        //
        // We need to fetch the run exports from the network.
        let middleware = self.client.as_ref()?;
        let run_exports_json_zst_url = subdir_url.join("run_exports.json.zst").ok()?;
        let request = middleware.get(run_exports_json_zst_url);
        let run_exports = if let Ok(response) = request.send().await {
            eprintln!("Downloading run exports from {}", response.url());
            let bytes_stream = response.bytes().await.ok()?;
            let buf = BufReader::new(bytes_stream.reader());
            let decoded = zstd::decode_all(buf).ok()?;
            match serde_json::from_slice(&decoded) {
                Ok(run_exports) => Some(run_exports),
                Err(e) => {
                    eprintln!("Failed to parse run exports: {}", e);
                    None
                }
            }
        } else {
            let run_exports_json_url = subdir_url.join("run_exports.json").ok()?;
            let request = middleware.get(run_exports_json_url);
            let response = request.send().await.ok()?;
            eprintln!("Downloading run exports from {}", response.url());
            response.json::<SubdirRunExportsJson>().await.ok()
        };

        // Package it up in an `Arc` so that it can be shared.
        let run_exports = run_exports.map(Arc::new);

        // Store the value in the cache.
        self.subdir_run_exports_cache.insert(
            subdir_url.clone(),
            PendingOrFetched::Fetched(run_exports.clone()),
        );

        // Notify any pending listeners.
        let _ = sender.send(run_exports.clone());

        run_exports
    }

    /// Extract the run exports from a package by first checking the
    /// `run_exports.json` file in the channel subdirectory, and if that fails,
    /// it will download the package to the cache and read the
    /// `run_exports.json` file from there.
    pub async fn extract(
        mut self,
        record: &RepoDataRecord,
        progress_reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        let platform_url = record.platform_url();

        // Try to fetch the `run_exports.json` from channel
        if let Some(subdir_run_exports) = self
            .fetch_subdir_run_exports(&platform_url, progress_reporter.clone())
            .await
        {
            return Ok(subdir_run_exports.get(record).cloned());
        }

        // Otherwise, fall back to extracting from the package cache.
        self.extract_into_package_cache(record, progress_reporter)
            .await
    }

    /// Extract the run exports from a package by downloading it to the cache
    /// and then reading the `run_exports.json` file.
    async fn extract_into_package_cache(
        self,
        record: &RepoDataRecord,
        progress_reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        let (Some(package_cache), Some(client)) =
            (self.package_cache.as_ref(), self.client.as_ref())
        else {
            return Ok(None);
        };

        let cache_key = CacheKey::from(&record.package_record);

        // Construct a reporter specifically for the run export download
        let reporter = progress_reporter
            .as_deref()
            .and_then(RunExportsReporter::create_package_download_reporter)
            .map(Arc::from);

        // Wait for a permit from the semaphore to limit concurrent requests.
        let _permit = OptionFuture::from(
            self.max_concurrent_requests
                .clone()
                .map(Semaphore::acquire_owned),
        )
        .await
        .transpose()
        .expect("semaphore error");

        match package_cache
            .get_or_fetch_from_url_with_retry(
                cache_key,
                record.url.clone(),
                client.clone(),
                default_retry_policy(),
                reporter,
            )
            .await
        {
            Ok(package_dir) => Ok(RunExportsJson::from_package_directory(package_dir.path()).ok()),
            Err(e) => Err(e.into()),
        }
    }
}
