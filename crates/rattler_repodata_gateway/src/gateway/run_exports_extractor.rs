use std::{io::BufReader, sync::Arc};

use bytes::Buf;
use dashmap::{DashMap, mapref::one::Ref};
use futures::future::OptionFuture;
use rattler_cache::package_cache::{CacheKey, CacheReporter, PackageCache, PackageCacheError};
use rattler_conda_types::{
    GlobalRunExportsJson, RepoDataRecord,
    package::{PackageFile, RunExportsJson},
};
use rattler_networking::retry_policies::default_retry_policy;
use reqwest_middleware::ClientWithMiddleware;
use thiserror::Error;
use tokio::sync::Semaphore;
use url::Url;

/// Simplest possible implementation of [`CacheReporter`].
#[derive(Default, Clone)]
pub struct DumpCacheReporter;

impl CacheReporter for DumpCacheReporter {
    fn on_validate_start(&self) -> usize {
        0
    }

    fn on_validate_complete(&self, _index: usize) {}

    fn on_download_start(&self) -> usize {
        0
    }

    fn on_download_progress(&self, _index: usize, _progress: u64, _total: Option<u64>) {}

    fn on_download_completed(&self, _index: usize) {}
}

/// Simplest possible implementation of [`RunExportsReporter`].
#[derive(Default, Clone)]
pub struct DumpPackageCacheReporter;

/// Reporter for multiple `RunExportsJson` retrieval.
pub trait RunExportsReporter {
    /// Adds a new package to the reporter. Returns a
    /// `PackageCacheReporterEntry` which can be passed to any of the
    /// cache function of a pacakage cache to track progress.
    fn add(&mut self, record: &RepoDataRecord) -> Arc<dyn CacheReporter>;
}

impl RunExportsReporter for DumpPackageCacheReporter {
    fn add(&mut self, _record: &RepoDataRecord) -> Arc<dyn CacheReporter> {
        Arc::new(DumpCacheReporter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrieveMethod {
    GlobalRunExportsJson(GlobalRunExportsJson),
    PackageRunExportsJson,
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
    retrieve_method_cache: Option<Arc<DashMap<Url, RetrieveMethod>>>,
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
    pub fn with_retrieve_method_cache(
        self,
        retrieve_method_cache: Arc<DashMap<Url, RetrieveMethod>>,
    ) -> Self {
        Self {
            retrieve_method_cache: Some(retrieve_method_cache),
            ..self
        }
    }

    /// Extracts the run exports from a package. Returns `None` if no run
    /// exports are found.
    pub async fn extract(
        mut self,
        record: &RepoDataRecord,
        progress_reporter: Arc<dyn CacheReporter>,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        self.extract_into_package_cache(record, progress_reporter)
            .await
    }

    async fn probe_global_run_exports(&self, platform_url: &Url) -> Option<GlobalRunExportsJson> {
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

    async fn probe_package_run_exports(&self, _platform_url: &Url) -> bool {
        // Maybe we actually want to do some checks?
        true
    }

    /// Probes channel for best available retrieve method
    async fn probe_retrieve_method(&self, url: &Url) -> RetrieveMethod {
        if let Some(global_run_exports_json) = self.probe_global_run_exports(url).await {
            RetrieveMethod::GlobalRunExportsJson(global_run_exports_json)
        } else if self.probe_package_run_exports(url).await {
            RetrieveMethod::PackageRunExportsJson
        } else {
            unreachable!();
        }
    }

    async fn insert_retrieve_method(&mut self, channel: &Url, reporter: Arc<dyn CacheReporter>) {
        if let Some(cache) = &self.retrieve_method_cache {
            if !cache.contains_key(channel) {
                let download_idx = reporter.on_download_start();
                let method = self.probe_retrieve_method(channel).await;
                cache.insert(channel.clone(), method);
                reporter.on_download_completed(download_idx);
            }
        }
    }

    pub async fn get_global_run_exports_json(
        &self,
        channel: &Url,
    ) -> Option<Ref<'_, Url, RetrieveMethod>> {
        self.retrieve_method_cache
            .as_ref()
            .map(|cache| cache.get(channel))?
    }

    /// Extract the run exports from a package by downloading it to the cache
    /// and then reading the `run_exports.json` file.
    async fn extract_into_package_cache(
        &mut self,
        record: &RepoDataRecord,
        progress_reporter: Arc<dyn CacheReporter>,
    ) -> Result<Option<RunExportsJson>, RunExportExtractorError> {
        let platform_url = record.platform_url();

        self.insert_retrieve_method(&platform_url, progress_reporter.clone())
            .await;

        let probably_global_run_exports = self.get_global_run_exports_json(&platform_url).await;

        let method = if let Some(global) = &probably_global_run_exports {
            global.value()
        } else {
            &RetrieveMethod::PackageRunExportsJson
        };

        let Some(package_cache) = self.package_cache.clone() else {
            return Ok(None);
        };

        match method {
            RetrieveMethod::GlobalRunExportsJson(global_run_exports_json) => {
                // TODO: Store in cache
                Ok(global_run_exports_json.get(record).cloned())
            }
            RetrieveMethod::PackageRunExportsJson => {
                let Some(client) = self.client.as_ref() else {
                    return Ok(None);
                };
                let cache_key = CacheKey::from(&record.package_record);
                let url = record.url.clone();
                let max_concurrent_requests = self.max_concurrent_requests.clone();

                let _permit =
                    OptionFuture::from(max_concurrent_requests.map(Semaphore::acquire_owned))
                        .await
                        .transpose()
                        .expect("semaphore error");

                match package_cache
                    .get_or_fetch_from_url_with_retry(
                        cache_key,
                        url,
                        client.clone(),
                        default_retry_policy(),
                        Some(progress_reporter),
                    )
                    .await
                {
                    Ok(package_dir) => {
                        Ok(RunExportsJson::from_package_directory(package_dir.path()).ok())
                    }
                    Err(e) => Err(e.into()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Semaphore;

    use super::*;
    use crate::Gateway;

    #[tokio::test]
    async fn test_probe_prefix() {
        let url = url::Url::parse("https://repo.prefix.dev/conda-forge/").unwrap();
        let platform_url = url.join("linux-64").unwrap();

        let gateway = Gateway::new();

        let max_concurrent_requests = Arc::new(Semaphore::new(1));
        let extractor = RunExportExtractor::default()
            .with_max_concurrent_requests(max_concurrent_requests.clone())
            .with_client(gateway.inner.client.clone())
            .with_package_cache(gateway.inner.package_cache.clone());

        assert!(matches!(
            extractor.probe_retrieve_method(&platform_url).await,
            RetrieveMethod::GlobalRunExportsJson(_)
        ),);
    }
}
