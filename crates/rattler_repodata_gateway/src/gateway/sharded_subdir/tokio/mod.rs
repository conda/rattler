mod index;

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use rattler_conda_types::Platform;

use super::{add_trailing_slash, decode_zst_bytes_async, parse_records};
use crate::{
    fetch::{CacheAction, FetchRepoDataError},
    gateway::{error::SubdirNotFoundError, subdir::SubdirClient},
    reporter::ResponseReporterExt,
    GatewayError, Reporter,
};
use fs_err::tokio as tokio_fs;
use futures::future::OptionFuture;
use http::{header::CACHE_CONTROL, HeaderValue, StatusCode};
use rattler_conda_types::{Channel, PackageName, RepoDataRecord, ShardedRepodata};
use rattler_networking::LazyClient;
use simple_spawn_blocking::tokio::run_blocking_task;
use url::Url;

const REPODATA_SHARDS_FILENAME: &str = "repodata_shards.msgpack.zst";

pub struct ShardedSubdir {
    channel: Channel,
    client: LazyClient,
    shards_base_url: Url,
    package_base_url: Url,
    sharded_repodata: ShardedRepodata,
    concurrent_requests_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    cache_dir: PathBuf,
    cache_action: CacheAction,
}

impl ShardedSubdir {
    pub async fn new(
        channel: Channel,
        subdir: String,
        client: LazyClient,
        cache_dir: PathBuf,
        cache_action: CacheAction,
        concurrent_requests_semaphore: Option<Arc<tokio::sync::Semaphore>>,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Self, GatewayError> {
        // Construct the base url for the shards (e.g. `<channel>/<subdir>`).
        let index_base_url = channel
            .base_url
            .url()
            .join(&format!("{subdir}/"))
            .expect("invalid subdir url");

        // Fetch the shard index
        let sharded_repodata = index::fetch_index(
            client.clone(),
            &index_base_url,
            &cache_dir,
            cache_action,
            concurrent_requests_semaphore.clone(),
            reporter,
        )
        .await
        .map_err(|e| match e {
            GatewayError::ReqwestError(e) if e.status() == Some(StatusCode::NOT_FOUND) => {
                GatewayError::SubdirNotFoundError(Box::new(SubdirNotFoundError {
                    channel: channel.clone(),
                    subdir,
                    source: e.into(),
                }))
            }
            e => e,
        })?;

        // Convert the URLs
        let shards_base_url = Url::options()
            .base_url(Some(&index_base_url))
            .parse(&sharded_repodata.info.shards_base_url)
            .map_err(|_e| {
                GatewayError::Generic(format!(
                    "shard index contains invalid `shards_base_url`: {}",
                    &sharded_repodata.info.shards_base_url
                ))
            })?;
        let package_base_url = Url::options()
            .base_url(Some(&index_base_url))
            .parse(&sharded_repodata.info.base_url)
            .map_err(|_e| {
                GatewayError::Generic(format!(
                    "shard index contains invalid `base_url`: {}",
                    &sharded_repodata.info.base_url
                ))
            })?;

        // Determine the cache directory and make sure it exists.
        let cache_dir = cache_dir.join("shards-v1");
        tokio_fs::create_dir_all(&cache_dir)
            .await
            .map_err(FetchRepoDataError::IoError)?;

        Ok(Self {
            channel,
            client,
            shards_base_url: add_trailing_slash(&shards_base_url).into_owned(),
            package_base_url: add_trailing_slash(&package_base_url).into_owned(),
            sharded_repodata,
            cache_dir,
            cache_action,
            concurrent_requests_semaphore,
        })
    }

    /// Clears the on-disk cache for the sharded repodata index of the given
    /// channel and platform.
    ///
    /// This acquires an exclusive lock on the cache file before removing it
    /// to prevent race conditions with concurrent readers/writers.
    ///
    /// If the cache file doesn't exist, this is a no-op since there's nothing
    /// to clear.
    pub fn clear_cache(
        cache_dir: &Path,
        channel: &Channel,
        platform: Platform,
    ) -> Result<(), std::io::Error> {
        let index_base_url = channel
            .base_url
            .url()
            .join(&format!("{}/", platform.as_str()))
            .expect("invalid subdir url");
        let canonical_shards_url = index_base_url
            .join(REPODATA_SHARDS_FILENAME)
            .expect("invalid shard base url");
        let cache_path = cache_dir.join(format!(
            "{}.shards-cache-v1",
            crate::utils::url_to_cache_filename(&canonical_shards_url)
        ));

        if cache_path.exists() {
            // Acquire an exclusive lock before removing the file.
            // This uses flock() on Unix (same as async_fd_lock used in normal flow).
            // On Unix, the file can be deleted while locked and will be removed
            // when the last handle is closed.
            let mut lock = fslock::LockFile::open(&cache_path).map_err(std::io::Error::other)?;
            lock.lock().map_err(std::io::Error::other)?;

            // Now remove the file while holding the lock
            fs_err::remove_file(&cache_path)?;
            tracing::debug!("deleted shard index cache: {:?}", cache_path);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SubdirClient for ShardedSubdir {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        // Find the shard that contains the package
        let Some(shard) = self.sharded_repodata.shards.get(name.as_normalized()) else {
            return Ok(vec![].into());
        };

        // Check if we already have the shard in the cache.
        let shard_cache_path = self.cache_dir.join(format!("{shard:x}.msgpack"));

        // Read the cached shard
        if self.cache_action != CacheAction::NoCache {
            match tokio_fs::read(&shard_cache_path).await {
                Ok(cached_bytes) => {
                    // Decode the cached shard
                    return parse_records(
                        cached_bytes,
                        self.channel.base_url.clone(),
                        self.package_base_url.clone(),
                    )
                    .await
                    .map(Arc::from);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    // The file is missing from the cache, we need to download
                    // it.
                }
                Err(err) => return Err(FetchRepoDataError::IoError(err).into()),
            }
        }

        if matches!(
            self.cache_action,
            CacheAction::UseCacheOnly | CacheAction::ForceCacheOnly
        ) {
            return Err(GatewayError::CacheError(format!(
                "the shard for package '{}' is not in the cache",
                name.as_source()
            )));
        }

        // Download the shard
        let shard_url = self
            .shards_base_url
            .join(&format!("{shard:x}.msgpack.zst"))
            .expect("invalid shard url");

        let shard_request = self
            .client
            .client()
            .get(shard_url.clone())
            .header(CACHE_CONTROL, HeaderValue::from_static("no-store"))
            .build()
            .expect("failed to build shard request");

        let shard_bytes = {
            let _request_permit = OptionFuture::from(
                self.concurrent_requests_semaphore
                    .as_deref()
                    .map(tokio::sync::Semaphore::acquire),
            )
            .await;
            let reporter = reporter
                .and_then(Reporter::download_reporter)
                .map(|r| (r, r.on_download_start(&shard_url)));
            let shard_response = self
                .client
                .client()
                .execute(shard_request)
                .await
                .and_then(|r| r.error_for_status().map_err(Into::into))
                .map_err(FetchRepoDataError::from)?;

            let bytes = shard_response
                .bytes_with_progress(reporter)
                .await
                .map_err(FetchRepoDataError::from)?;

            if let Some((reporter, index)) = reporter {
                reporter.on_download_complete(&shard_url, index);
            }

            bytes
        };

        let shard_bytes = decode_zst_bytes_async(shard_bytes).await?;

        // Create a future to write the cached bytes to disk
        let write_to_cache_fut = write_shard_to_cache(shard_cache_path, shard_bytes.clone());

        // Create a future to parse the records from the shard
        let parse_records_fut = parse_records(
            shard_bytes,
            self.channel.base_url.clone(),
            self.package_base_url.clone(),
        );

        // Await both futures concurrently.
        let (_, records) = tokio::try_join!(write_to_cache_fut, parse_records_fut)?;

        Ok(records.into())
    }

    fn package_names(&self) -> Vec<String> {
        self.sharded_repodata.shards.keys().cloned().collect()
    }
}

/// Atomically writes the shard bytes to the cache.
async fn write_shard_to_cache(
    shard_cache_path: PathBuf,
    shard_bytes: Vec<u8>,
) -> Result<(), GatewayError> {
    run_blocking_task(move || {
        let shard_cache_parent_path = shard_cache_path
            .parent()
            .expect("file path must have a parent");
        let mut temp_file = tempfile::Builder::new()
            .tempfile_in(
                shard_cache_path
                    .parent()
                    .expect("file path must have a parent"),
            )
            .map_err(|e| {
                GatewayError::IoError(
                    format!(
                        "failed to create temporary file to write shard in {}",
                        shard_cache_parent_path.display()
                    ),
                    e,
                )
            })?;
        temp_file.write_all(&shard_bytes).map_err(|e| {
            GatewayError::IoError(
                format!(
                    "failed to write shard to temporary file in {}",
                    shard_cache_parent_path.display()
                ),
                e,
            )
        })?;
        match temp_file.persist(&shard_cache_path) {
            Ok(_) => Ok(()),
            Err(e) => {
                if shard_cache_path.is_file() {
                    // The file already exists, we can ignore the error.
                    Ok(())
                } else {
                    Err(GatewayError::IoError(
                        format!("failed to persist shard to {}", shard_cache_path.display()),
                        e.error,
                    ))
                }
            }
        }
    })
    .await
}
