use std::{collections::HashMap, io::BufReader, sync::Arc};

use bytes::Buf;
use futures::future::OptionFuture;
use rattler_cache::package_cache::{CacheKey, CacheReporter, PackageCache, PackageCacheError};
use rattler_conda_types::{
    package::{PackageFile, RunExportsJson},
    GlobalRunExportsJson, RepoDataRecord,
};
use rattler_networking::retry_policies::default_retry_policy;
use reqwest_middleware::ClientWithMiddleware;
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore};
use url::Url;

/// Type used for in-memory caching of `GlobalRunExportsJson`.
pub type GlobalRunExportsCache = Arc<Mutex<HashMap<Url, Option<GlobalRunExportsJson>>>>;

/// Reporter for multiple `RunExportsJson` retrievals.
pub trait RunExportsReporter: Send + Sync {
    /// Adds a new package to the reporter. Returns a
    /// `PackageCacheReporterEntry` which can be passed to any of the
    /// cache function of a package cache to track progress.
    fn add(&self, record: &RepoDataRecord) -> Arc<dyn CacheReporter>;
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
    global_run_exports_cache: Option<GlobalRunExportsCache>,
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
    pub fn with_max_concurrent_requests(self, max_concurrent_requests: Arc<Semaphore>) -> Self {
        Self {
            max_concurrent_requests: Some(max_concurrent_requests),
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
        global_run_exports_cache: GlobalRunExportsCache,
    ) -> Self {
        Self {
            global_run_exports_cache: Some(global_run_exports_cache),
            ..self
        }
    }

    /// Extracts the run exports from a package. Returns `None` if no run
    /// exports are found.
    pub async fn extract(
        self,
        record: &RepoDataRecord,
        progress_reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        self.extract_into_package_cache(record, progress_reporter)
            .await
    }

    pub(crate) async fn probe_global_run_exports(
        &self,
        platform_url: &Url,
    ) -> Option<GlobalRunExportsJson> {
        let middleware = self.client.as_ref()?;
        let run_exports_json_zst_url = platform_url.join("run_exports.json.zst").ok()?;
        let request = middleware.get(run_exports_json_zst_url);
        if let Ok(response) = request.send().await {
            let bytes_stream = response.bytes().await.ok()?;
            let buf = BufReader::new(bytes_stream.reader());
            let decoded = zstd::decode_all(buf).ok()?;
            serde_json::from_slice(&decoded).ok()
        } else {
            let run_exports_json_url = platform_url.join("run_exports.json").ok()?;
            let request = middleware.get(run_exports_json_url);
            let response = request.send().await.ok()?;
            response.json::<GlobalRunExportsJson>().await.ok()
        }
    }

    async fn insert_global_run_exports(
        &mut self,
        channel: &Url,
        reporter: Option<Arc<dyn CacheReporter>>,
    ) {
        if let Some(cache) = &self.global_run_exports_cache {
            let mut cache_lock = cache.lock().await;
            if !cache_lock.contains_key(channel) {
                let download_idx = reporter.as_ref().map(|r| r.on_download_start());
                let method = self.probe_global_run_exports(channel).await;
                cache_lock.insert(channel.clone(), method);

                if let Some(idx) = download_idx {
                    reporter.as_ref().unwrap().on_download_completed(idx);
                }
            }
        }
    }

    /// Extract the run exports from a package by downloading it to the cache
    /// and then reading the `run_exports.json` file.
    async fn extract_into_package_cache(
        mut self,
        record: &RepoDataRecord,
        progress_reporter: Option<Arc<dyn CacheReporter>>,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        let platform_url = record.platform_url();

        self.insert_global_run_exports(&platform_url, progress_reporter.clone())
            .await;

        if let Some(global_run_exports_cache) = self.global_run_exports_cache.clone() {
            let lock = global_run_exports_cache.lock().await;

            if let Some(Some(global_run_exports)) = lock.get(&platform_url) {
                return Ok(global_run_exports.get(record).cloned());
            }
        }

        let Some(package_cache) = self.package_cache.clone() else {
            return Ok(None);
        };

        let Some(client) = self.client.as_ref() else {
            return Ok(None);
        };
        let cache_key = CacheKey::from(&record.package_record);
        let url = record.url.clone();
        let max_concurrent_requests = self.max_concurrent_requests.clone();

        let _permit = OptionFuture::from(max_concurrent_requests.map(Semaphore::acquire_owned))
            .await
            .transpose()
            .expect("semaphore error");

        match package_cache
            .get_or_fetch_from_url_with_retry(
                cache_key,
                url,
                client.clone(),
                default_retry_policy(),
                progress_reporter,
            )
            .await
        {
            Ok(package_dir) => Ok(RunExportsJson::from_package_directory(package_dir.path()).ok()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rattler_conda_types::utils::url_with_trailing_slash::UrlWithTrailingSlash;
    use tokio::sync::Semaphore;

    use super::*;
    use crate::Gateway;

    #[tokio::test]
    async fn test_probe_prefix() {
        let url = url::Url::parse("https://repo.prefix.dev/conda-forge/").unwrap();
        let platform_url = UrlWithTrailingSlash::from(url.join("linux-64/").unwrap());

        let gateway = Gateway::new();

        let cache = Arc::new(Mutex::new(HashMap::new()));

        let max_concurrent_requests = Arc::new(Semaphore::new(1));
        let extractor = RunExportExtractor::default()
            .with_max_concurrent_requests(max_concurrent_requests.clone())
            .with_client(gateway.inner.client.clone())
            .with_package_cache(gateway.inner.package_cache.clone())
            .with_global_run_exports_cache(cache);

        assert!((extractor.probe_global_run_exports(&platform_url).await).is_some());
    }
}
