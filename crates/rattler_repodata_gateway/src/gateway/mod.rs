mod barrier_cell;
mod builder;
mod channel_config;
#[cfg(not(target_arch = "wasm32"))]
mod direct_url_query;
mod error;
#[cfg(feature = "indicatif")]
mod indicatif;
mod local_subdir;
mod query;
mod remote_subdir;
mod repo_data;
mod run_exports_extractor;
mod sharded_subdir;
mod subdir;
mod subdir_builder;

use std::{collections::HashSet, sync::Arc};

use crate::{gateway::subdir_builder::SubdirBuilder, Reporter};
pub use barrier_cell::BarrierCell;
pub use builder::{GatewayBuilder, MaxConcurrency};
pub use channel_config::{ChannelConfig, SourceConfig};
use coalesced_map::{CoalescedGetError, CoalescedMap};
pub use error::GatewayError;
#[cfg(feature = "indicatif")]
pub use indicatif::{IndicatifReporter, IndicatifReporterBuilder};
pub use query::{NamesQuery, RepoDataQuery};
#[cfg(not(target_arch = "wasm32"))]
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{Channel, MatchSpec, Platform, RepoDataRecord};
use rattler_networking::LazyClient;
pub use repo_data::RepoData;
use run_exports_extractor::{RunExportExtractor, SubdirRunExportsCache};
pub use run_exports_extractor::{RunExportExtractorError, RunExportsReporter};
use subdir::Subdir;
use tracing::{instrument, Level};
use url::Url;

/// Central access point for high level queries about
/// [`rattler_conda_types::RepoDataRecord`]s from different channels.
///
/// The gateway is responsible for fetching and caching repodata. Requests are
/// deduplicated which means that if multiple requests are made for the same
/// repodata only the first request will actually fetch the data. All other
/// requests will wait for the first request to complete and then return the
/// same data.
///
/// The gateway is thread-safe and can be shared between multiple threads. The
/// gateway struct itself uses internal reference counting and is cheaply
/// cloneable. There is no need to wrap the gateway in an `Arc`.
#[derive(Clone)]
pub struct Gateway {
    inner: Arc<GatewayInner>,
}

impl Default for Gateway {
    fn default() -> Self {
        Gateway::new()
    }
}

/// A selection of subdirectories.
#[derive(Default, Clone, Debug)]
pub enum SubdirSelection {
    /// Select all subdirectories
    #[default]
    All,

    /// Select these specific subdirectories
    Some(HashSet<String>),
}

impl SubdirSelection {
    /// Returns `true` if the given subdirectory is part of the selection.
    pub fn contains(&self, subdir: &str) -> bool {
        match self {
            SubdirSelection::All => true,
            SubdirSelection::Some(subdirs) => subdirs.contains(&subdir.to_string()),
        }
    }
}

/// Specifies which cache layers to clear.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheClearMode {
    /// Clear only the in-memory cache
    #[default]
    InMemory,

    /// Clear both in-memory and on-disk cache
    #[cfg(not(target_arch = "wasm32"))]
    InMemoryAndDisk,
}

impl Gateway {
    /// Constructs a simple gateway with the default configuration. Use
    /// [`Gateway::builder`] if you want more control over how the gateway
    /// is constructed.
    pub fn new() -> Self {
        Gateway::builder().finish()
    }

    /// Constructs a new gateway with the given client and channel
    /// configuration.
    pub fn builder() -> GatewayBuilder {
        GatewayBuilder::default()
    }

    /// Constructs a new `GatewayQuery` which can be used to query repodata
    /// records.
    pub fn query<AsChannel, ChannelIter, PlatformIter, PackageNameIter, IntoMatchSpec>(
        &self,
        channels: ChannelIter,
        platforms: PlatformIter,
        specs: PackageNameIter,
    ) -> RepoDataQuery
    where
        AsChannel: Into<Channel>,
        ChannelIter: IntoIterator<Item = AsChannel>,
        PlatformIter: IntoIterator<Item = Platform>,
        <PlatformIter as IntoIterator>::IntoIter: Clone,
        PackageNameIter: IntoIterator<Item = IntoMatchSpec>,
        IntoMatchSpec: Into<MatchSpec>,
    {
        RepoDataQuery::new(
            self.inner.clone(),
            channels.into_iter().map(Into::into).collect(),
            platforms.into_iter().collect(),
            specs.into_iter().map(Into::into).collect(),
        )
    }

    /// Return all names from repodata
    pub fn names<AsChannel, ChannelIter, PlatformIter>(
        &self,
        channels: ChannelIter,
        platforms: PlatformIter,
    ) -> NamesQuery
    where
        AsChannel: Into<Channel>,
        ChannelIter: IntoIterator<Item = AsChannel>,
        PlatformIter: IntoIterator<Item = Platform>,
        <PlatformIter as IntoIterator>::IntoIter: Clone,
    {
        NamesQuery::new(
            self.inner.clone(),
            channels.into_iter().map(Into::into).collect(),
            platforms.into_iter().collect(),
        )
    }

    /// Ensure that given repodata records contain `RunExportsJson`.
    pub async fn ensure_run_exports(
        &self,
        records: impl Iterator<Item = &mut RepoDataRecord>,
        // We can avoid Arc by cloning, but this requires helper method in the trait definition.
        progress_reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<(), RunExportExtractorError> {
        let futures = records
            .filter_map(|record| {
                if record.package_record.run_exports.is_some() {
                    // If the package already has run exports, we don't need to do anything.
                    return None;
                }

                let extractor = RunExportExtractor::default()
                    .with_opt_max_concurrent_requests(
                        self.inner.concurrent_requests_semaphore.clone(),
                    )
                    .with_client(self.inner.client.clone())
                    .with_global_run_exports_cache(self.inner.subdir_run_exports_cache.clone());

                #[cfg(not(target_arch = "wasm32"))]
                let extractor = extractor.with_package_cache(self.inner.package_cache.clone());

                let progress_reporter = progress_reporter.clone();
                Some(async move {
                    extractor
                        .extract(record, progress_reporter)
                        .await
                        .map(|rexp| (record, rexp))
                })
            })
            .collect::<Vec<_>>();

        let results = futures::future::try_join_all(futures).await?;

        for (record, result) in results {
            record.package_record.run_exports = result;
        }

        Ok(())
    }

    /// Clears cache for the given channel.
    ///
    /// Any subsequent query will re-fetch any required data from the source.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel to clear the cache for
    /// * `subdirs` - Which subdirectories to clear (all or specific platforms)
    /// * `mode` - Whether to clear only in-memory cache or both in-memory and on-disk cache
    ///
    /// # Behavior
    ///
    /// When using [`CacheClearMode::InMemoryAndDisk`], this function will:
    /// - Acquire file locks for each cache entry before deletion
    /// - Block and wait if locks are held by other processes
    /// - Delete cache files (`.json`, `.info.json`) while holding the lock
    /// - Leave lock files in place to prevent ABA locking problems
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rattler_repodata_gateway::{Gateway, SubdirSelection, CacheClearMode};
    /// use rattler_conda_types::Channel;
    /// use url::Url;
    ///
    /// let gateway = Gateway::new();
    /// let channel = Channel::from_url(
    ///     Url::parse("https://conda.anaconda.org/conda-forge").unwrap()
    /// );
    ///
    /// // Clear only in-memory cache (fast, always available)
    /// gateway.clear_repodata_cache(
    ///     &channel,
    ///     SubdirSelection::All,
    ///     CacheClearMode::InMemory
    /// )?;
    ///
    /// # #[cfg(not(target_arch = "wasm32"))]
    /// # {
    /// // Clear both in-memory and on-disk cache (may block on locks)
    /// gateway.clear_repodata_cache(
    ///     &channel,
    ///     SubdirSelection::All,
    ///     CacheClearMode::InMemoryAndDisk
    /// )?;
    /// # }
    /// # Ok::<(), std::io::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if `mode` is [`CacheClearMode::InMemoryAndDisk`] and any file deletion fails.
    /// Continues attempting to delete remaining files even if some deletions fail.
    #[allow(unused_variables)]
    pub fn clear_repodata_cache(
        &self,
        channel: &Channel,
        subdirs: SubdirSelection,
        mode: CacheClearMode,
    ) -> Result<(), std::io::Error> {
        // Clear in-memory cache
        self.inner.subdirs.retain(|key, _| {
            key.0.base_url != channel.base_url || !subdirs.contains(key.1.as_str())
        });

        // Clear on-disk cache if requested
        #[cfg(not(target_arch = "wasm32"))]
        if mode == CacheClearMode::InMemoryAndDisk {
            use crate::utils::LockedFile;
            use std::str::FromStr;

            let cache_dir = &self.inner.cache;

            // For each platform that matches the selection, compute the cache keys and delete the files
            let platforms_to_clear: Vec<Platform> = match &subdirs {
                SubdirSelection::All => {
                    // Get all known platforms
                    Platform::all().collect()
                }
                SubdirSelection::Some(subdirs) => {
                    // Parse the platform names from the subdir strings
                    subdirs
                        .iter()
                        .filter_map(|s| Platform::from_str(s).ok())
                        .collect()
                }
            };

            let mut errors = Vec::new();

            for platform in platforms_to_clear {
                // Construct the subdir URL for this channel and platform
                let subdir_url = channel.platform_url(platform);

                // Compute cache keys for both regular and current repodata variants
                for variant in [
                    crate::fetch::Variant::AfterPatches,
                    crate::fetch::Variant::Current,
                ] {
                    let repodata_url = match subdir_url.join(variant.file_name()) {
                        Ok(url) => url,
                        Err(e) => {
                            tracing::warn!("Failed to join URL: {}", e);
                            continue;
                        }
                    };

                    let cache_key = crate::utils::url_to_cache_filename(&repodata_url);

                    // Acquire the lock file to ensure exclusive access before deleting.
                    // LockedFile::open_rw will block and wait until the lock is available,
                    // ensuring we don't delete files that are currently being used by another process.
                    let lock_file_path = cache_dir.join(format!("{}.lock", &cache_key));
                    let _lock = match LockedFile::open_rw(&lock_file_path, "repodata cache cleanup")
                    {
                        Ok(lock) => {
                            tracing::debug!("Acquired lock for {:?}", lock_file_path);
                            lock
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to acquire lock for {:?}: {}. Skipping deletion.",
                                lock_file_path,
                                e
                            );
                            errors.push(std::io::Error::other(format!(
                                "Failed to acquire lock: {e}"
                            )));
                            continue;
                        }
                    };

                    // Delete the cache files (json and info.json) while holding the lock
                    for extension in ["json", "info.json"] {
                        let file_path = cache_dir.join(format!("{cache_key}.{extension}"));
                        if file_path.exists() {
                            if let Err(e) = fs_err::remove_file(&file_path) {
                                tracing::warn!(
                                    "Failed to delete cache file {:?}: {}",
                                    file_path,
                                    e
                                );
                                errors.push(e);
                            } else {
                                tracing::debug!("Deleted cache file: {:?}", file_path);
                            }
                        }
                    }

                    // Drop the lock explicitly before deleting the lock file itself
                    drop(_lock);

                    // Note: we do not delete the lock file to prevent a ABA "locking" problem where
                    // another process might acquire the lock between us dropping it and deleting the file.
                }
            }

            if let Some(first_error) = errors.into_iter().next() {
                return Err(first_error);
            }
        }

        Ok(())
    }
}

struct GatewayInner {
    /// A map of subdirectories for each channel and platform.
    subdirs: CoalescedMap<(Channel, Platform), Arc<Subdir>>,

    /// The client to use to fetch repodata.
    client: LazyClient,

    /// The channel configuration
    channel_config: ChannelConfig,

    /// The directory to store any cache
    #[cfg(not(target_arch = "wasm32"))]
    cache: std::path::PathBuf,

    /// The package cache, stored to reuse memory cache
    #[cfg(not(target_arch = "wasm32"))]
    package_cache: PackageCache,

    /// A cache for global run exports.
    subdir_run_exports_cache: Arc<SubdirRunExportsCache>,

    /// A semaphore to limit the number of concurrent requests.
    concurrent_requests_semaphore: Option<Arc<tokio::sync::Semaphore>>,
}

impl GatewayInner {
    /// Returns the [`Subdir`] for the given channel and platform. This
    /// function will create the [`Subdir`] if it does not exist yet, otherwise
    /// it will return the previously created subdir.
    ///
    /// If multiple threads request the same subdir their requests will be
    /// coalesced, and they will all receive the same subdir. If an error
    /// occurs while creating the subdir all waiting tasks will also return an
    /// error.
    #[instrument(skip(self, reporter, channel), fields(channel = %channel.base_url), err(level = Level::INFO))]
    async fn get_or_create_subdir(
        &self,
        channel: &Channel,
        platform: Platform,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Arc<Subdir>, GatewayError> {
        let key = (channel.clone(), platform);
        let channel = channel.clone();

        self.subdirs
            .get_or_try_init(key, || async move {
                let subdir = self.create_subdir(&channel, platform, reporter).await?;
                Ok(Arc::new(subdir))
            })
            .await
            .map_err(|e| match e {
                CoalescedGetError::Init(gateway_err) => gateway_err,
                CoalescedGetError::CoalescedRequestFailed => GatewayError::IoError(
                    "a coalesced request failed".to_string(),
                    std::io::ErrorKind::Other.into(),
                ),
            })
    }

    async fn create_subdir(
        &self,
        channel: &Channel,
        platform: Platform,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Subdir, GatewayError> {
        SubdirBuilder::new(self, channel.clone(), platform, reporter)
            .build()
            .await
    }
}

fn force_sharded_repodata(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && matches!(url.host_str(), Some("fast.prefiks.dev" | "fast.prefix.dev"))
}

#[cfg(test)]
mod test {
    use std::{
        path::{Path, PathBuf},
        str::FromStr,
        sync::Arc,
        time::Instant,
    };

    use assert_matches::assert_matches;
    use dashmap::DashSet;
    use rattler_cache::{default_cache_dir, package_cache::PackageCache};
    use rattler_conda_types::{
        Channel, ChannelConfig, MatchSpec, PackageName,
        ParseStrictness::{Lenient, Strict},
        Platform, RepoDataRecord,
    };
    use rstest::rstest;
    use url::Url;

    use crate::{
        fetch::CacheAction, gateway::Gateway, utils::simple_channel_server::SimpleChannelServer,
        DownloadReporter, GatewayError, JLAPReporter, RepoData, Reporter, SourceConfig,
        SubdirSelection,
    };

    async fn local_conda_forge() -> Channel {
        tokio::try_join!(
            tools::fetch_test_conda_forge_repodata_async("noarch"),
            tools::fetch_test_conda_forge_repodata_async("linux-64")
        )
        .unwrap();
        Channel::from_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/channels/conda-forge"),
        )
    }

    async fn remote_conda_forge() -> SimpleChannelServer {
        tokio::try_join!(
            tools::fetch_test_conda_forge_repodata_async("noarch"),
            tools::fetch_test_conda_forge_repodata_async("linux-64")
        )
        .unwrap();
        SimpleChannelServer::new(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/channels/conda-forge"),
        )
        .await
    }

    #[tokio::test]
    async fn test_local_gateway() {
        let gateway = Gateway::new();

        let records = gateway
            .query(
                vec![local_conda_forge().await],
                vec![Platform::Linux64, Platform::NoArch],
                vec![PackageName::from_str("rubin-env").unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();
        assert_eq!(total_records, 45060);
    }

    #[tokio::test]
    async fn test_remote_gateway() {
        let gateway = Gateway::new();

        let index = remote_conda_forge().await;

        let records = gateway
            .query(
                vec![index.channel()],
                vec![Platform::Linux64, Platform::Win32, Platform::NoArch],
                vec![PackageName::from_str("rubin-env").unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();
        assert_eq!(total_records, 45060);
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn test_direct_url_spec_from_gateway() {
        let gateway = Gateway::builder()
            .with_package_cache(PackageCache::new(
                default_cache_dir()
                    .unwrap()
                    .join(rattler_cache::PACKAGE_CACHE_DIR),
            ))
            .with_cache_dir(
                default_cache_dir()
                    .unwrap()
                    .join(rattler_cache::REPODATA_CACHE_DIR),
            )
            .finish();

        let index = local_conda_forge().await;

        let records = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Win64],
                vec![MatchSpec::from_str(
                    "https://conda.anaconda.org/conda-forge/win-64/openssl-3.3.1-h2466b09_1.conda",
                    Strict,
                )
                .unwrap()]
                .into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let non_openssl_direct_records = records
            .iter()
            .flat_map(RepoData::iter)
            .filter(|record| record.package_record.name.as_normalized() != "openssl")
            .collect::<Vec<_>>()
            .len();

        let records = gateway
            .query(
                vec![index],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("openssl ==3.3.1 h2466b09_1", Strict).unwrap()]
                    .into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let non_openssl_total_records = records
            .iter()
            .flat_map(RepoData::iter)
            .filter(|record| record.package_record.name.as_normalized() != "openssl")
            .collect::<Vec<_>>()
            .len();

        // The total records without the matchspec should be the same.
        assert_eq!(non_openssl_total_records, non_openssl_direct_records);
    }

    // Make sure that the direct url version of openssl is used instead of the one
    // from the normal channel.
    #[tokio::test]
    async fn test_select_forced_url_instead_of_deps() {
        let gateway = Gateway::builder()
            .with_package_cache(PackageCache::new(
                default_cache_dir()
                    .unwrap()
                    .join(rattler_cache::PACKAGE_CACHE_DIR),
            ))
            .with_cache_dir(
                default_cache_dir()
                    .unwrap()
                    .join(rattler_cache::REPODATA_CACHE_DIR),
            )
            .finish();

        let index = local_conda_forge().await;

        let openssl_url =
            "https://conda.anaconda.org/conda-forge/win-64/openssl-3.3.1-h2466b09_1.conda";
        let records = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Linux64],
                vec![
                    MatchSpec::from_str("mamba ==0.9.2 py39h951de11_0", Strict).unwrap(),
                    MatchSpec::from_str(openssl_url, Strict).unwrap(),
                ]
                .into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let total_records_single_openssl: usize = records.iter().map(RepoData::len).sum();
        assert_eq!(total_records_single_openssl, 4219);

        // There should be only one record for the openssl package.
        let openssl_records: Vec<&RepoDataRecord> = records
            .iter()
            .flat_map(RepoData::iter)
            .filter(|record| record.package_record.name.as_normalized() == "openssl")
            .collect();
        assert_eq!(openssl_records.len(), 1);

        // Test if the first repodata subdir contains only the direct url package.
        let first_subdir = records.first().unwrap();
        assert_eq!(first_subdir.len, 1);
        let openssl_record = first_subdir
            .iter()
            .find(|record| record.package_record.name.as_normalized() == "openssl")
            .unwrap();
        assert_eq!(openssl_record.url.as_str(), openssl_url);

        // ------------------------------------------------------------
        // Now we query for the openssl package without the direct url.
        // ------------------------------------------------------------
        let gateway = Gateway::new();
        let records = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("mamba ==0.9.2 py39h951de11_0", Strict).unwrap()]
                    .into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();

        // The total number of records should be greater than the number of records
        // fetched when selecting the openssl with a direct url.
        assert!(total_records > total_records_single_openssl);
        assert_eq!(total_records, 4267);

        let openssl_records: Vec<&RepoDataRecord> = records
            .iter()
            .flat_map(RepoData::iter)
            .filter(|record| record.package_record.name.as_normalized() == "openssl")
            .collect();
        assert!(openssl_records.len() > 1);
    }

    #[tokio::test]
    async fn test_filter_with_specs() {
        let gateway = Gateway::new();

        let index = local_conda_forge().await;

        // Try a complex spec
        let matchspec = MatchSpec::from_str("openssl=3.*=*_1", Lenient).unwrap();

        let records = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Linux64],
                vec![matchspec].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();
        assert!(total_records == 3);

        let _repodata_records = records
            .iter()
            .flat_map(|r| r.iter().cloned())
            .collect::<Vec<_>>();

        // Try another spec
        let matchspec = MatchSpec::from_str("openssl=3", Lenient).unwrap();

        let records = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Linux64],
                vec![matchspec].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();
        assert!(total_records == 9);

        // Try with multiple specs
        let matchspec1 = MatchSpec::from_str("openssl=3", Lenient).unwrap();
        let matchspec2 = MatchSpec::from_str("openssl=1", Lenient).unwrap();

        let records = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Linux64],
                vec![matchspec1, matchspec2].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();
        assert!(total_records == 49);
    }

    #[tokio::test]
    async fn test_nameless_matchspec_error() {
        let gateway = Gateway::new();

        let index = local_conda_forge().await;

        let mut matchspec = MatchSpec::from_str(
            "https://conda.anaconda.org/conda-forge/linux-64/openssl-3.0.4-h166bdaf_2.tar.bz2",
            Strict,
        )
        .unwrap();
        matchspec.name = None;

        let gateway_error = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Linux64],
                vec![matchspec].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap_err();

        assert_matches!(gateway_error, GatewayError::MatchSpecWithoutExactName(_));
    }

    #[rstest]
    #[case::named("non-existing-channel")]
    #[case::url("https://conda.anaconda.org/does-not-exist")]
    #[case::file_url("file:///does/not/exist")]
    #[case::win_path("c:/does-not-exist")]
    #[case::unix_path("/does-not-exist")]
    #[tokio::test]
    async fn test_doesnt_exist(#[case] channel: &str) {
        let gateway = Gateway::new();

        let default_channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());
        let err = gateway
            .query(
                vec![Channel::from_str(channel, &default_channel_config).unwrap()],
                vec![Platform::Linux64, Platform::NoArch],
                vec![PackageName::from_str("some-package").unwrap()].into_iter(),
            )
            .await;

        assert_matches::assert_matches!(err, Err(GatewayError::SubdirNotFoundError(_)));
    }

    #[ignore]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_sharded_gateway() {
        let gateway = Gateway::new();

        let start = Instant::now();
        let records = gateway
            .query(
                vec![Channel::from_url(
                    Url::parse("https://conda.anaconda.org/conda-forge").unwrap(),
                )],
                vec![Platform::Linux64, Platform::NoArch],
                vec![
                    // PackageName::from_str("rubin-env").unwrap(),
                    // PackageName::from_str("jupyterlab").unwrap(),
                    // PackageName::from_str("detectron2").unwrap(),
                    PackageName::from_str("python").unwrap(),
                    PackageName::from_str("boto3").unwrap(),
                    PackageName::from_str("requests").unwrap(),
                ]
                .into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();
        let end = Instant::now();
        println!("{} records in {:?}", records.len(), end - start);

        let total_records: usize = records.iter().map(RepoData::len).sum();
        assert_eq!(total_records, 84242);
    }

    #[tokio::test]
    async fn test_clear_cache() {
        #[derive(Default)]
        struct Downloads {
            urls: DashSet<Url>,
        }
        impl DownloadReporter for Arc<Downloads> {
            fn on_download_complete(&self, url: &Url, _index: usize) {
                self.urls.insert(url.clone());
            }
        }
        impl Reporter for Arc<Downloads> {
            fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
                Some(self)
            }
            fn jlap_reporter(&self) -> Option<&dyn JLAPReporter> {
                None
            }
        }

        let local_channel = remote_conda_forge().await;

        // Create a gateway with a custom channel configuration that disables caching.
        let gateway = Gateway::builder()
            .with_channel_config(super::ChannelConfig {
                default: SourceConfig {
                    cache_action: CacheAction::NoCache,
                    ..Default::default()
                },
                ..Default::default()
            })
            .finish();

        let downloads = Arc::new(Downloads::default());

        // Construct a simple query
        let query = gateway
            .query(
                vec![local_channel.channel()],
                vec![Platform::Linux64, Platform::NoArch],
                vec![PackageName::from_str("python").unwrap()].into_iter(),
            )
            .with_reporter(downloads.clone());

        // Run the query once. We expect some activity.
        query.clone().execute().await.unwrap();
        assert!(!downloads.urls.is_empty(), "there should be some urls");
        downloads.urls.clear();

        // Run the query a second time.
        query.clone().execute().await.unwrap();
        assert!(
            downloads.urls.is_empty(),
            "there should be NO new url fetches"
        );

        // Now clear the cache and run the query again.
        gateway
            .clear_repodata_cache(
                &local_channel.channel(),
                SubdirSelection::default(),
                super::CacheClearMode::InMemory,
            )
            .unwrap();
        query.clone().execute().await.unwrap();
        assert!(
            !downloads.urls.is_empty(),
            "after clearing the cache there should be new urls fetched"
        );
    }

    #[tokio::test]
    async fn test_clear_cache_from_disk() {
        use std::path::PathBuf;
        use tempfile::TempDir;

        let local_channel = remote_conda_forge().await;

        // Create a gateway with a temporary cache directory
        let cache_dir = TempDir::new().unwrap();
        let gateway = Gateway::builder()
            .with_cache_dir(cache_dir.path().to_path_buf())
            .finish();

        // Run a query to populate the cache
        let _records = gateway
            .query(
                vec![local_channel.channel()],
                vec![Platform::Linux64, Platform::NoArch],
                vec![PackageName::from_str("python").unwrap()].into_iter(),
            )
            .execute()
            .await
            .unwrap();

        // Verify that cache files were created
        let cache_files: Vec<PathBuf> = std::fs::read_dir(cache_dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "json" || ext == "lock")
            })
            .collect();

        assert!(
            !cache_files.is_empty(),
            "there should be cache files on disk"
        );

        // Clear the cache from disk
        gateway
            .clear_repodata_cache(
                &local_channel.channel(),
                SubdirSelection::default(),
                super::CacheClearMode::InMemoryAndDisk,
            )
            .unwrap();

        // Verify that cache files were deleted
        let remaining_cache_files: Vec<PathBuf> = std::fs::read_dir(cache_dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "json" || ext == "lock")
            })
            .collect();

        assert!(
            remaining_cache_files.is_empty(),
            "all cache files should be deleted"
        );
    }

    fn run_exports_missing(records: &[RepoDataRecord]) -> bool {
        records
            .iter()
            .any(|rr| rr.package_record.run_exports.is_none())
    }

    fn run_exports_in_place(records: &[RepoDataRecord]) -> bool {
        records.iter().all(|rr| {
            let Some(run_exports) = &rr.package_record.run_exports else {
                return false;
            };
            !run_exports.is_empty()
        })
    }

    #[tokio::test]
    async fn test_ensure_run_exports_local_conda_forge() {
        let gateway = Gateway::new();

        let index = local_conda_forge().await;

        // Try a complex spec
        let matchspec = MatchSpec::from_str("openssl=3.*=*_1", Lenient).unwrap();

        let records = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Linux64],
                vec![matchspec].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();
        assert_eq!(total_records, 3);

        let mut repodata_records = records
            .iter()
            .flat_map(|r| r.iter().cloned())
            .collect::<Vec<_>>();

        assert!(run_exports_missing(&repodata_records));

        gateway
            .ensure_run_exports(repodata_records.iter_mut(), None)
            .await
            .unwrap();

        assert!(run_exports_in_place(&repodata_records));
    }

    #[tokio::test]
    async fn test_ensure_run_exports_remote_conda_forge() {
        let gateway = Gateway::new();

        let records = gateway
            .query(
                vec![Channel::from_url(
                    Url::parse("https://conda.anaconda.org/conda-forge/").unwrap(),
                )],
                vec![Platform::Linux64, Platform::NoArch],
                vec![MatchSpec::from_str("openssl=3.*=*_1", Lenient).unwrap()].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();
        assert_eq!(total_records, 16);

        let mut repodata_records = records
            .iter()
            .flat_map(|r| r.iter().cloned())
            .collect::<Vec<_>>();

        assert!(run_exports_missing(&repodata_records));

        gateway
            .ensure_run_exports(repodata_records.iter_mut(), None)
            .await
            .unwrap();

        assert!(run_exports_in_place(&repodata_records));
    }
}
