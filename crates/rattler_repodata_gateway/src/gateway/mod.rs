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
mod source;
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
pub use source::{RepoDataSource, Source};
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

/// Specifies what caches to clear.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheClearMode {
    /// Only clear in-memory caches.
    #[default]
    InMemoryOnly,

    /// Clear both in-memory and on-disk caches.
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
    ///
    /// # Sources
    ///
    /// The `sources` parameter accepts any type that implements `Into<Source>`.
    /// This includes:
    /// - `Channel` - traditional conda channels
    /// - `Arc<dyn RepoDataSource>` - custom repodata sources
    /// - `Source` - the enum itself
    ///
    /// Existing code using channels continues to work unchanged:
    ///
    /// ```ignore
    /// gateway.query(
    ///     vec![channel1, channel2],
    ///     vec![Platform::Linux64],
    ///     vec![spec],
    /// ).await
    /// ```
    ///
    /// You can also mix channels and custom sources:
    ///
    /// ```ignore
    /// gateway.query(
    ///     vec![
    ///         Source::Channel(channel),
    ///         Source::Custom(my_custom_source),
    ///     ],
    ///     vec![Platform::Linux64],
    ///     vec![spec],
    /// ).await
    /// ```
    pub fn query<AsSource, SourceIter, PlatformIter, PackageNameIter, IntoMatchSpec>(
        &self,
        sources: SourceIter,
        platforms: PlatformIter,
        specs: PackageNameIter,
    ) -> RepoDataQuery
    where
        AsSource: Into<Source>,
        SourceIter: IntoIterator<Item = AsSource>,
        PlatformIter: IntoIterator<Item = Platform>,
        <PlatformIter as IntoIterator>::IntoIter: Clone,
        PackageNameIter: IntoIterator<Item = IntoMatchSpec>,
        IntoMatchSpec: Into<MatchSpec>,
    {
        RepoDataQuery::new(
            self.inner.clone(),
            sources.into_iter().map(Into::into).collect(),
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

    /// Clears any in-memory cache for the given channel.
    ///
    /// Any subsequent query will re-fetch any required data from the source.
    ///
    /// When `mode` is [`CacheClearMode::InMemoryAndDisk`], this method also
    /// clears on-disk caches for the specified channel and subdirectories.
    pub fn clear_repodata_cache(
        &self,
        channel: &Channel,
        subdirs: SubdirSelection,
        mode: CacheClearMode,
    ) -> Result<(), std::io::Error> {
        self.inner.subdirs.retain(|key, _| {
            key.0.base_url != channel.base_url || !subdirs.contains(key.1.as_str())
        });

        #[cfg(not(target_arch = "wasm32"))]
        if mode == CacheClearMode::InMemoryAndDisk {
            use std::str::FromStr;

            let platforms_to_clear: Vec<Platform> = match &subdirs {
                SubdirSelection::All => Platform::all().collect(),
                SubdirSelection::Some(subdirs) => subdirs
                    .iter()
                    .filter_map(|s| Platform::from_str(s).ok())
                    .collect(),
            };

            let mut errors = Vec::new();
            for platform in platforms_to_clear {
                if let Err(e) = remote_subdir::RemoteSubdirClient::clear_cache(
                    &self.inner.cache,
                    channel,
                    platform,
                ) {
                    errors.push(e);
                }
                if let Err(e) =
                    sharded_subdir::ShardedSubdir::clear_cache(&self.inner.cache, channel, platform)
                {
                    errors.push(e);
                }
            }
            if let Some(first_error) = errors.into_iter().next() {
                return Err(first_error);
            }
        }

        #[cfg(target_arch = "wasm32")]
        let _ = mode;

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
                super::CacheClearMode::InMemoryOnly,
            )
            .unwrap();
        query.clone().execute().await.unwrap();
        assert!(
            !downloads.urls.is_empty(),
            "after clearing the cache there should be new urls fetched"
        );
    }

    #[test]
    fn test_clear_disk_cache() {
        use crate::gateway::remote_subdir::RemoteSubdirClient;

        let cache_dir = tempfile::tempdir().unwrap();

        // Create a test channel
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());
        let channel = Channel::from_str("conda-forge", &channel_config).unwrap();

        // Create mock cache files for linux-64 platform
        let subdir_url = channel.platform_url(Platform::Linux64);
        let cache_key = crate::utils::url_to_cache_filename(
            &subdir_url.join("repodata.json").expect("valid filename"),
        );

        // Create mock cache files
        let json_path = cache_dir.path().join(format!("{cache_key}.json"));
        let info_path = cache_dir.path().join(format!("{cache_key}.info.json"));
        let lock_path = cache_dir.path().join(format!("{cache_key}.lock"));

        std::fs::write(&json_path, b"{}").unwrap();
        std::fs::write(&info_path, b"{}").unwrap();
        std::fs::write(&lock_path, b"").unwrap();

        // Verify files exist
        assert!(json_path.exists(), "json file should exist before clear");
        assert!(info_path.exists(), "info file should exist before clear");
        assert!(lock_path.exists(), "lock file should exist before clear");

        // Clear the disk cache
        RemoteSubdirClient::clear_cache(cache_dir.path(), &channel, Platform::Linux64).unwrap();

        // Verify json and info files are removed but lock file remains
        assert!(
            !json_path.exists(),
            "json file should be removed after clear"
        );
        assert!(
            !info_path.exists(),
            "info file should be removed after clear"
        );
        assert!(
            lock_path.exists(),
            "lock file should remain after clear to avoid ABA problem"
        );
    }

    #[test]
    fn test_clear_sharded_disk_cache() {
        use crate::gateway::sharded_subdir::{
            ShardedSubdir, REPODATA_SHARDS_FILENAME, SHARDS_CACHE_SUFFIX,
        };

        let cache_dir = tempfile::tempdir().unwrap();

        // Create a test channel
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());
        let channel = Channel::from_str("conda-forge", &channel_config).unwrap();

        // Create mock sharded cache file for linux-64 platform
        let index_base_url = channel
            .base_url
            .url()
            .join(&format!("{}/", Platform::Linux64.as_str()))
            .expect("invalid subdir url");
        let canonical_shards_url = index_base_url
            .join(REPODATA_SHARDS_FILENAME)
            .expect("invalid shard base url");
        let cache_path = cache_dir.path().join(format!(
            "{}{}",
            crate::utils::url_to_cache_filename(&canonical_shards_url),
            SHARDS_CACHE_SUFFIX
        ));

        // Create mock cache file
        std::fs::write(&cache_path, b"mock shard data").unwrap();

        // Verify file exists
        assert!(
            cache_path.exists(),
            "sharded cache file should exist before clear"
        );

        // Clear the disk cache
        ShardedSubdir::clear_cache(cache_dir.path(), &channel, Platform::Linux64).unwrap();

        // Verify cache file is removed
        assert!(
            !cache_path.exists(),
            "sharded cache file should be removed after clear"
        );
    }

    #[test]
    fn test_clear_disk_cache_no_cache() {
        use crate::gateway::remote_subdir::RemoteSubdirClient;

        let cache_dir = tempfile::tempdir().unwrap();

        // Create a test channel
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());
        let channel = Channel::from_str("conda-forge", &channel_config).unwrap();

        // Clear should succeed even when there's no cache (empty directory)
        RemoteSubdirClient::clear_cache(cache_dir.path(), &channel, Platform::Linux64).unwrap();

        // Clear should also succeed when the cache directory doesn't exist at all
        let non_existent_dir = cache_dir.path().join("does-not-exist");
        RemoteSubdirClient::clear_cache(&non_existent_dir, &channel, Platform::Linux64).unwrap();
    }

    #[test]
    fn test_clear_sharded_disk_cache_no_cache() {
        use crate::gateway::sharded_subdir::ShardedSubdir;

        let cache_dir = tempfile::tempdir().unwrap();

        // Create a test channel
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());
        let channel = Channel::from_str("conda-forge", &channel_config).unwrap();

        // Clear should succeed even when there's no cache (empty directory)
        ShardedSubdir::clear_cache(cache_dir.path(), &channel, Platform::Linux64).unwrap();

        // Clear should also succeed when the cache directory doesn't exist at all
        let non_existent_dir = cache_dir.path().join("does-not-exist");
        ShardedSubdir::clear_cache(&non_existent_dir, &channel, Platform::Linux64).unwrap();
    }

    #[test]
    fn test_gateway_clear_repodata_cache() {
        let cache_dir = tempfile::tempdir().unwrap();

        // Create a test channel
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());
        let channel = Channel::from_str("conda-forge", &channel_config).unwrap();

        // Create a gateway with the custom cache directory
        let gateway = Gateway::builder()
            .with_cache_dir(cache_dir.path().to_path_buf())
            .finish();

        // Clear should succeed even when there's no cache
        gateway
            .clear_repodata_cache(
                &channel,
                SubdirSelection::default(),
                super::CacheClearMode::InMemoryAndDisk,
            )
            .unwrap();

        // Clear with specific subdirs should also succeed
        gateway
            .clear_repodata_cache(
                &channel,
                SubdirSelection::Some(
                    ["linux-64", "noarch"]
                        .into_iter()
                        .map(String::from)
                        .collect(),
                ),
                super::CacheClearMode::InMemoryAndDisk,
            )
            .unwrap();

        // Clear in-memory only should also succeed
        gateway
            .clear_repodata_cache(
                &channel,
                SubdirSelection::default(),
                super::CacheClearMode::InMemoryOnly,
            )
            .unwrap();
    }

    /// Helper function to generate minimal repodata JSON for a single package.
    fn make_repodata(name: &str, version: &str) -> String {
        format!(
            r#"{{
    "packages.conda": {{
        "{name}-{version}-0.conda": {{
            "build": "0",
            "build_number": 0,
            "depends": [],
            "md5": "00000000000000000000000000000000",
            "name": "{name}",
            "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "size": 1000,
            "subdir": "linux-64",
            "timestamp": 1700000000000,
            "version": "{version}"
        }}
    }}
}}"#
        )
    }

    /// Integration test that verifies cache clearing actually works end-to-end.
    /// Creates a simple channel with a single package, queries it, modifies
    /// the source data, and verifies that memory-only cache clearing still
    /// returns cached data while disk cache clearing forces a re-fetch.
    #[tokio::test]
    async fn test_gateway_clear_disk_cache_integration() {
        // Create a temporary directory for the channel subdir
        let channel_dir = tempfile::tempdir().unwrap();
        let subdir_path = channel_dir.path().join("linux-64");
        std::fs::create_dir_all(&subdir_path).unwrap();

        // Write initial repodata with version 1.0.0
        let repodata_v1 = make_repodata("testpkg", "1.0.0");
        std::fs::write(subdir_path.join("repodata.json"), &repodata_v1).unwrap();

        // Start the SimpleChannelServer
        let server = SimpleChannelServer::new(channel_dir.path()).await;
        let channel = server.channel();

        // Create a temporary cache directory
        let cache_dir = tempfile::tempdir().unwrap();

        // Create a gateway with the custom cache directory
        let gateway = Gateway::builder()
            .with_cache_dir(cache_dir.path().to_path_buf())
            .finish();

        // Query to populate the cache - should get version 1.0.0
        let records = gateway
            .query(
                vec![channel.clone()],
                vec![Platform::Linux64],
                vec![PackageName::from_str("testpkg").unwrap()].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        // Verify we got exactly one record with version 1.0.0
        let all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();
        assert_eq!(all_records.len(), 1, "should have exactly one record");
        assert_eq!(
            all_records[0].package_record.version.as_str(),
            "1.0.0",
            "initial version should be 1.0.0"
        );

        // Sleep to ensure filesystem timestamp changes (server uses mtime for caching)
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

        // Modify the repodata on disk - change to version 2.0.0
        let repodata_v2 = make_repodata("testpkg", "2.0.0");
        std::fs::write(subdir_path.join("repodata.json"), &repodata_v2).unwrap();

        // Query again without any cache clearing - should still get 1.0.0 (memory cache)
        let records = gateway
            .query(
                vec![channel.clone()],
                vec![Platform::Linux64],
                vec![PackageName::from_str("testpkg").unwrap()].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();
        assert_eq!(
            all_records[0].package_record.version.as_str(),
            "1.0.0",
            "should still be 1.0.0 due to memory cache"
        );

        // Clear memory cache only (not disk)
        gateway
            .clear_repodata_cache(
                &channel,
                SubdirSelection::Some(["linux-64"].into_iter().map(String::from).collect()),
                super::CacheClearMode::InMemoryOnly,
            )
            .unwrap();

        // Create a new gateway with ForceCacheOnly to verify disk cache has v1
        // (Using ForceCacheOnly ensures we read from disk cache without checking server)
        let gateway_cache_only = Gateway::builder()
            .with_cache_dir(cache_dir.path().to_path_buf())
            .with_channel_config(crate::ChannelConfig {
                default: SourceConfig {
                    cache_action: CacheAction::ForceCacheOnly,
                    ..Default::default()
                },
                ..Default::default()
            })
            .finish();

        // Query with ForceCacheOnly - should get 1.0.0 from disk cache
        let records = gateway_cache_only
            .query(
                vec![channel.clone()],
                vec![Platform::Linux64],
                vec![PackageName::from_str("testpkg").unwrap()].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();
        assert_eq!(
            all_records[0].package_record.version.as_str(),
            "1.0.0",
            "should still be 1.0.0 from disk cache when using ForceCacheOnly"
        );

        // Clear both memory and disk cache on the original gateway
        gateway
            .clear_repodata_cache(
                &channel,
                SubdirSelection::Some(["linux-64"].into_iter().map(String::from).collect()),
                super::CacheClearMode::InMemoryAndDisk,
            )
            .unwrap();

        // Query again (fresh gateway to avoid memory cache)
        // should now get 2.0.0 (fresh fetch from server since disk cache was cleared)
        let gateway_fresh = Gateway::builder()
            .with_cache_dir(cache_dir.path().to_path_buf())
            .finish();

        let records = gateway_fresh
            .query(
                vec![channel.clone()],
                vec![Platform::Linux64],
                vec![PackageName::from_str("testpkg").unwrap()].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();
        assert_eq!(
            all_records[0].package_record.version.as_str(),
            "2.0.0",
            "should now be 2.0.0 after clearing disk cache"
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
        assert_eq!(total_records, 19);

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

    /// A mock `RepoDataSource` for testing custom source functionality.
    struct MockRepoDataSource {
        records: std::collections::HashMap<(Platform, PackageName), Vec<RepoDataRecord>>,
    }

    impl MockRepoDataSource {
        fn new() -> Self {
            Self {
                records: std::collections::HashMap::new(),
            }
        }

        fn add_record(&mut self, platform: Platform, record: RepoDataRecord) {
            let name = record.package_record.name.clone();
            self.records
                .entry((platform, name))
                .or_default()
                .push(record);
        }
    }

    #[async_trait::async_trait]
    impl super::RepoDataSource for MockRepoDataSource {
        async fn fetch_package_records(
            &self,
            platform: Platform,
            name: &PackageName,
        ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
            let records = self
                .records
                .get(&(platform, name.clone()))
                .cloned()
                .unwrap_or_default();
            Ok(Arc::from(records))
        }

        fn package_names(&self, platform: Platform) -> Vec<String> {
            self.records
                .keys()
                .filter(|(p, _)| *p == platform)
                .map(|(_, n)| n.as_source().to_string())
                .collect()
        }
    }

    fn make_test_record(name: &str, version: &str, subdir: &str) -> RepoDataRecord {
        use rattler_conda_types::{
            package::DistArchiveIdentifier, PackageRecord, VersionWithSource,
        };

        let package_record = PackageRecord {
            name: PackageName::from_str(name).unwrap(),
            version: VersionWithSource::from_str(version).unwrap(),
            build: "0".to_string(),
            build_number: 0,
            subdir: subdir.to_string(),
            md5: None,
            sha256: None,
            size: Some(1000),
            arch: None,
            platform: None,
            depends: vec![],
            constrains: vec![],
            track_features: vec![],
            features: None,
            noarch: rattler_conda_types::NoArchType::default(),
            license: None,
            license_family: None,
            timestamp: None,
            legacy_bz2_size: None,
            legacy_bz2_md5: None,
            purls: None,
            run_exports: None,
            python_site_packages_path: None,
            experimental_extra_depends: std::collections::BTreeMap::default(),
        };

        RepoDataRecord {
            package_record,
            identifier: format!("{name}-{version}-0.conda")
                .parse::<DistArchiveIdentifier>()
                .unwrap(),
            url: Url::parse(&format!(
                "https://example.com/{subdir}/{name}-{version}-0.conda"
            ))
            .unwrap(),
            channel: Some("example".to_string()),
        }
    }

    #[tokio::test]
    async fn test_custom_source() {
        let gateway = Gateway::new();

        // Create a mock source with some records
        let mut mock_source = MockRepoDataSource::new();
        mock_source.add_record(
            Platform::Linux64,
            make_test_record("testpkg", "1.0.0", "linux-64"),
        );
        mock_source.add_record(
            Platform::Linux64,
            make_test_record("testpkg", "2.0.0", "linux-64"),
        );
        mock_source.add_record(
            Platform::NoArch,
            make_test_record("otherpkg", "1.0.0", "noarch"),
        );

        let source: Arc<dyn super::RepoDataSource> = Arc::new(mock_source);

        // Query using the custom source
        let records = gateway
            .query(
                vec![super::Source::Custom(source.clone())],
                vec![Platform::Linux64],
                vec![PackageName::from_str("testpkg").unwrap()].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();
        assert_eq!(all_records.len(), 2, "should have two testpkg records");

        // Check versions
        let versions: std::collections::HashSet<_> = all_records
            .iter()
            .map(|r| r.package_record.version.as_str())
            .collect();
        assert!(versions.contains("1.0.0"));
        assert!(versions.contains("2.0.0"));

        // Query for noarch platform
        let records = gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::NoArch],
                vec![PackageName::from_str("otherpkg").unwrap()].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();
        assert_eq!(all_records.len(), 1, "should have one otherpkg record");
        assert_eq!(
            all_records[0].package_record.name.as_normalized(),
            "otherpkg"
        );
    }

    #[tokio::test]
    async fn test_mixed_channel_and_custom_source() {
        let gateway = Gateway::new();

        // Create a mock source with a custom package
        let mut mock_source = MockRepoDataSource::new();
        mock_source.add_record(
            Platform::Linux64,
            make_test_record("custom-pkg", "1.0.0", "linux-64"),
        );

        let custom_source: Arc<dyn super::RepoDataSource> = Arc::new(mock_source);

        // Get the local conda-forge channel
        let channel = local_conda_forge().await;

        // Query both the channel and custom source
        let records = gateway
            .query(
                vec![
                    super::Source::Channel(channel),
                    super::Source::Custom(custom_source),
                ],
                vec![Platform::Linux64],
                vec![
                    PackageName::from_str("python").unwrap(),
                    PackageName::from_str("custom-pkg").unwrap(),
                ]
                .into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        // We should have results from both sources
        let all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();

        // Check we got python from the channel
        let python_records: Vec<_> = all_records
            .iter()
            .filter(|r| r.package_record.name.as_normalized() == "python")
            .collect();
        assert!(
            !python_records.is_empty(),
            "should have python from channel"
        );

        // Check we got custom-pkg from our mock source
        let custom_records: Vec<_> = all_records
            .iter()
            .filter(|r| r.package_record.name.as_normalized() == "custom-pkg")
            .collect();
        assert_eq!(
            custom_records.len(),
            1,
            "should have custom-pkg from mock source"
        );
        assert_eq!(custom_records[0].package_record.version.as_str(), "1.0.0");
    }

    /// Test that ensures run_exports fallback works when run_exports.json exists
    /// but doesn't contain all packages (out-of-sync scenario).
    ///
    /// This test creates a channel that has an empty run_exports.json file,
    /// simulating the case where the run_exports.json file is out of sync with
    /// the actual packages. The test verifies that the system correctly falls
    /// back to extracting run_exports from the actual package files.
    #[tokio::test]
    async fn test_ensure_run_exports_fallback_when_out_of_sync() {
        // Use a minimal repodata with just one openssl package, and add a base_url
        // pointing to conda.anaconda.org so fallback downloads work.
        let channel_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/channels/openssl-run-exports-test");

        // Set up the test server
        let server = SimpleChannelServer::new(channel_dir).await;
        let gateway = Gateway::new();

        // Query for openssl packages (which have run_exports)
        let matchspec = MatchSpec::from_str("openssl=3.3.1", Lenient).unwrap();

        let records = gateway
            .query(
                vec![server.channel()],
                vec![Platform::Linux64],
                vec![matchspec].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();
        assert!(total_records > 0, "should find some openssl packages");

        let mut repodata_records = records
            .iter()
            .flat_map(|r| r.iter().cloned())
            .collect::<Vec<_>>();

        // Run exports should be missing initially since we just queried repodata
        assert!(run_exports_missing(&repodata_records));

        // Now ensure run_exports - this should:
        // 1. Fetch the empty run_exports.json from our test server
        // 2. Not find the packages in run_exports.json (it's empty)
        // 3. Fall back to downloading the actual packages and extracting run_exports
        gateway
            .ensure_run_exports(repodata_records.iter_mut(), None)
            .await
            .unwrap();

        // Verify that run_exports were successfully extracted via fallback
        assert!(
            run_exports_in_place(&repodata_records),
            "run_exports should be populated via package extraction fallback when run_exports.json is out of sync"
        );
    }
}
