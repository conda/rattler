mod barrier_cell;
mod builder;
mod channel_config;
mod channel_expander;
mod channel_relations;
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
mod warning;

use std::{collections::HashSet, sync::Arc};

use crate::reporter::report_unsupported_repodata_revisions;
use crate::{Reporter, gateway::subdir_builder::SubdirBuilder};
pub use barrier_cell::BarrierCell;
pub use builder::{GatewayBuilder, MaxConcurrency};
pub use channel_config::{ChannelConfig, SourceConfig};
pub use channel_expander::{ChannelRelationsMode, ChannelRelationsWarning};
pub use channel_relations::DEFAULT_CHANNEL_RELATIONS_MAX_DEPTH;
use coalesced_map::{CoalescedGetError, CoalescedMap};
pub use error::GatewayError;
#[cfg(feature = "indicatif")]
pub use indicatif::{IndicatifReporter, IndicatifReporterBuilder};
pub use query::{NamesQuery, NamesQueryOutput, RepoDataQuery, RepoDataQueryOutput};
#[cfg(not(target_arch = "wasm32"))]
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{Channel, ChannelRelations, MatchSpec, Platform, RepoDataRecord};
use rattler_networking::LazyClient;
pub use repo_data::RepoData;
use run_exports_extractor::{RunExportExtractor, SubdirRunExportsCache};
pub use run_exports_extractor::{RunExportExtractorError, RunExportsReporter};
pub use source::{RepoDataSource, Source};
use subdir::Subdir;
use tracing::{Level, instrument};
use url::Url;
pub use warning::GatewayWarning;

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

    /// Returns the [CEP-42] `channel_relations` declared by the given
    /// `(channel, platform)` subdirectory, or `None` if none were
    /// declared or the subdirectory doesn't exist.
    ///
    /// Reuses the internal subdir cache: if the pair has already been
    /// fetched by a [`Gateway::query`] this is free.
    ///
    /// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
    pub async fn channel_relations(
        &self,
        channel: &Channel,
        platform: Platform,
    ) -> Result<Option<ChannelRelations>, GatewayError> {
        let subdir = self
            .inner
            .get_or_create_subdir(channel, platform, None)
            .await?;
        Ok(subdir.channel_relations().cloned())
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
        let channel_for_create = channel.clone();
        let reporter_for_create = reporter.clone();

        let subdir = self
            .subdirs
            .get_or_try_init(key, || async move {
                let subdir = self
                    .create_subdir(&channel_for_create, platform, reporter_for_create)
                    .await?;
                Ok(Arc::new(subdir))
            })
            .await
            .map_err(|e| match e {
                CoalescedGetError::Init(gateway_err) => gateway_err,
                CoalescedGetError::CoalescedRequestFailed => GatewayError::IoError(
                    "a coalesced request failed".to_string(),
                    std::io::ErrorKind::Other.into(),
                ),
            })?;

        report_unsupported_repodata_revisions(
            reporter.as_deref(),
            channel,
            platform.as_str(),
            subdir.repodata_revisions(),
        );

        Ok(subdir)
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
        sync::{Arc, Mutex},
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
        DownloadReporter, GatewayError, RepoData, Reporter, SourceConfig, SubdirSelection,
        UnsupportedRepodataRevision, fetch::CacheAction, gateway::Gateway,
        utils::simple_channel_server::SimpleChannelServer,
    };
    use rattler_conda_types::RepodataRevision;

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
    async fn test_unsupported_repodata_revision_reporter() {
        #[derive(Default)]
        struct RevisionReporter {
            messages: Mutex<Vec<UnsupportedRepodataRevision>>,
        }

        impl Reporter for Arc<RevisionReporter> {
            fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
                None
            }

            fn on_unsupported_repodata_revision(&self, message: &UnsupportedRepodataRevision) {
                self.messages.lock().unwrap().push(message.clone());
            }
        }

        let tempdir = tempfile::tempdir().unwrap();
        let noarch = tempdir.path().join("noarch");
        fs_err::create_dir_all(&noarch).unwrap();
        fs_err::write(
            noarch.join("repodata.json"),
            r#"{
                "repodata_version": 1,
                "info": {
                    "subdir": "noarch",
                    "repodata_revisions": {
                        "v4": {
                            "n_packages": 2,
                            "oldest": 1768249989851,
                            "newest": 1773851561010
                        }
                    }
                },
                "packages": {},
                "packages.conda": {
                    "demo-1.0-0.conda": {
                        "build": "0",
                        "build_number": 0,
                        "depends": [],
                        "md5": "82ecc40f09b9c44483e6b70cad2545d7",
                        "name": "demo",
                        "noarch": "generic",
                        "sha256": "eb65e866067865793b981c2ba74485f75bef441842b5998badc4ec66717685c7",
                        "size": 1234,
                        "subdir": "noarch",
                        "timestamp": 1689209309623,
                        "version": "1.0"
                    }
                }
            }"#,
        )
        .unwrap();

        let reporter = Arc::new(RevisionReporter::default());
        let gateway = Gateway::new();
        let channel = Channel::from_directory(tempdir.path());
        let records = gateway
            .query(
                vec![channel.clone()],
                vec![Platform::NoArch],
                vec![PackageName::from_str("demo").unwrap()],
            )
            .with_reporter(reporter.clone())
            .await
            .unwrap();

        assert_eq!(records.iter().map(RepoData::len).sum::<usize>(), 1);

        // The message is reported from cached subdirs too, so callers can
        // attach a reporter per query and still surface the warning.
        let records = gateway
            .query(
                vec![channel],
                vec![Platform::NoArch],
                vec![PackageName::from_str("demo").unwrap()],
            )
            .with_reporter(reporter.clone())
            .await
            .unwrap();

        assert_eq!(records.iter().map(RepoData::len).sum::<usize>(), 1);
        let messages = reporter.messages.lock().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].subdir, "noarch");
        assert_eq!(messages[0].supported_revision, RepodataRevision::V3);
        assert_eq!(messages[0].revision.revision, RepodataRevision::Unknown(4));
        assert_eq!(messages[0].revision.n_packages, Some(2));
        assert_eq!(messages[1], messages[0]);
    }

    #[tokio::test]
    async fn test_supported_repodata_revision_reporter_ignored() {
        #[derive(Default)]
        struct RevisionReporter {
            messages: Mutex<Vec<UnsupportedRepodataRevision>>,
        }

        impl Reporter for Arc<RevisionReporter> {
            fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
                None
            }

            fn on_unsupported_repodata_revision(&self, message: &UnsupportedRepodataRevision) {
                self.messages.lock().unwrap().push(message.clone());
            }
        }

        let tempdir = tempfile::tempdir().unwrap();
        let noarch = tempdir.path().join("noarch");
        fs_err::create_dir_all(&noarch).unwrap();
        fs_err::write(
            noarch.join("repodata.json"),
            r#"{
                "repodata_version": 1,
                "info": {
                    "subdir": "noarch",
                    "repodata_revisions": {
                        "v3": {
                            "n_packages": 1
                        }
                    }
                },
                "packages": {},
                "packages.conda": {
                    "demo-1.0-0.conda": {
                        "build": "0",
                        "build_number": 0,
                        "depends": [],
                        "md5": "82ecc40f09b9c44483e6b70cad2545d7",
                        "name": "demo",
                        "noarch": "generic",
                        "sha256": "eb65e866067865793b981c2ba74485f75bef441842b5998badc4ec66717685c7",
                        "size": 1234,
                        "subdir": "noarch",
                        "timestamp": 1689209309623,
                        "version": "1.0"
                    }
                }
            }"#,
        )
        .unwrap();

        let reporter = Arc::new(RevisionReporter::default());
        let records = Gateway::new()
            .query(
                vec![Channel::from_directory(tempdir.path())],
                vec![Platform::NoArch],
                vec![PackageName::from_str("demo").unwrap()],
            )
            .with_reporter(reporter.clone())
            .await
            .unwrap();

        assert_eq!(records.iter().map(RepoData::len).sum::<usize>(), 1);
        let messages = reporter.messages.lock().unwrap();
        assert!(messages.is_empty());
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
        assert_eq!(first_subdir.len(), 1);
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
        matchspec.name = "*".parse().expect("wildcard always parses");

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
            REPODATA_SHARDS_FILENAME, SHARDS_CACHE_SUFFIX, ShardedSubdir,
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
        // conda-forge's sharded repodata now embeds `run_exports` directly in the
        // records. Disable sharded repodata so that the records are fetched from
        // `repodata.json` (which does not contain `run_exports`). This ensures the
        // records start out without `run_exports` and allows us to exercise
        // `ensure_run_exports`.
        let gateway = Gateway::builder()
            .with_channel_config(crate::ChannelConfig {
                default: SourceConfig {
                    sharded_enabled: false,
                    ..SourceConfig::default()
                },
                ..crate::ChannelConfig::default()
            })
            .finish();

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
        ) -> Result<Vec<Arc<RepoDataRecord>>, GatewayError> {
            let records = self
                .records
                .get(&(platform, name.clone()))
                .cloned()
                .unwrap_or_default();
            Ok(records.into_iter().map(Arc::new).collect())
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
        make_test_record_full(name, version, subdir, &[], &[])
    }

    fn make_test_record_full(
        name: &str,
        version: &str,
        subdir: &str,
        depends: &[&str],
        extra_depends: &[(&str, &[&str])],
    ) -> RepoDataRecord {
        use rattler_conda_types::{
            PackageRecord, VersionWithSource, package::DistArchiveIdentifier,
        };

        let mut extra_depends_map: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::default();
        for (extra, items) in extra_depends {
            extra_depends_map.insert(
                (*extra).to_string(),
                items.iter().map(|s| (*s).to_string()).collect(),
            );
        }

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
            depends: depends.iter().map(|s| (*s).to_string()).collect(),
            constrains: vec![],
            track_features: vec![],
            features: None,
            flags: vec![],
            noarch: rattler_conda_types::NoArchType::default(),
            license: None,
            license_family: None,
            timestamp: None,
            legacy_bz2_size: None,
            legacy_bz2_md5: None,
            purls: None,
            run_exports: None,
            python_site_packages_path: None,
            extra_depends: extra_depends_map,
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

    /// Test that ensures `run_exports` fallback works when `run_exports.json` exists
    /// but doesn't contain all packages (out-of-sync scenario).
    ///
    /// This test creates a channel that has an empty `run_exports.json` file,
    /// simulating the case where the `run_exports.json` file is out of sync with
    /// the actual packages. The test verifies that the system correctly falls
    /// back to extracting `run_exports` from the actual package files.
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

    #[tokio::test]
    async fn test_glob_pattern_query() {
        use rattler_conda_types::ParseStrictnessWithNameMatcher;

        let gateway = Gateway::new();

        let index = local_conda_forge().await;

        // Query with glob pattern "openssl*" - should match packages starting with
        // "openssl"
        let matchspec = MatchSpec::from_str(
            "openssl*",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();

        let records = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Linux64],
                vec![matchspec].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();

        // Verify we got some results
        assert!(
            !all_records.is_empty(),
            "glob pattern should match some packages"
        );

        // Verify all results start with "openssl"
        for record in &all_records {
            assert!(
                record
                    .package_record
                    .name
                    .as_normalized()
                    .starts_with("openssl"),
                "all matched packages should start with 'openssl', got: {}",
                record.package_record.name.as_normalized()
            );
        }
    }

    #[tokio::test]
    async fn test_regex_pattern_query() {
        use rattler_conda_types::ParseStrictnessWithNameMatcher;

        let gateway = Gateway::new();

        let index = local_conda_forge().await;

        // Query with regex pattern - match packages starting with "python-"
        // Regex patterns are enclosed in ^...$ or use regex syntax
        let matchspec = MatchSpec::from_str(
            "^python-.*$",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();

        let records = gateway
            .query(
                vec![index.clone()],
                vec![Platform::Linux64],
                vec![matchspec].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();

        // Verify we got some results
        assert!(
            !all_records.is_empty(),
            "regex pattern should match some packages"
        );

        // Verify all results match the pattern (start with "python-")
        for record in &all_records {
            assert!(
                record
                    .package_record
                    .name
                    .as_normalized()
                    .starts_with("python-"),
                "all matched packages should start with 'python-', got: {}",
                record.package_record.name.as_normalized()
            );
        }
    }

    #[tokio::test]
    async fn test_glob_pattern_query_no_matches() {
        use rattler_conda_types::ParseStrictnessWithNameMatcher;

        let gateway = Gateway::new();

        let index = local_conda_forge().await;

        // Query with glob pattern that matches nothing
        let matchspec = MatchSpec::from_str(
            "zzznonexistent*",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();

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
        assert_eq!(
            total_records, 0,
            "glob pattern with no matches should return empty results"
        );
    }

    /// Verify that glob pattern queries discover matching packages across
    /// multiple channels *and* multiple subdirs, even when each source
    /// carries a disjoint set of package names.
    #[tokio::test]
    async fn test_glob_pattern_across_multiple_channels_and_subdirs() {
        use rattler_conda_types::ParseStrictnessWithNameMatcher;

        let gateway = Gateway::new();

        // --- Source A: has lib-foo on linux-64 and lib-bar on noarch ----------
        let mut source_a = MockRepoDataSource::new();
        source_a.add_record(
            Platform::Linux64,
            make_test_record("lib-foo", "1.0.0", "linux-64"),
        );
        source_a.add_record(
            Platform::NoArch,
            make_test_record("lib-bar", "2.0.0", "noarch"),
        );
        // A non-matching package that should be excluded.
        source_a.add_record(
            Platform::Linux64,
            make_test_record("unrelated", "1.0.0", "linux-64"),
        );

        // --- Source B: has lib-baz on linux-64 and lib-qux on noarch ---------
        let mut source_b = MockRepoDataSource::new();
        source_b.add_record(
            Platform::Linux64,
            make_test_record("lib-baz", "3.0.0", "linux-64"),
        );
        source_b.add_record(
            Platform::NoArch,
            make_test_record("lib-qux", "4.0.0", "noarch"),
        );
        // Another non-matching package.
        source_b.add_record(
            Platform::NoArch,
            make_test_record("other-pkg", "1.0.0", "noarch"),
        );

        let src_a: Arc<dyn super::RepoDataSource> = Arc::new(source_a);
        let src_b: Arc<dyn super::RepoDataSource> = Arc::new(source_b);

        // Glob that matches every "lib-*" package.
        let matchspec = MatchSpec::from_str(
            "lib-*",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();

        let records = gateway
            .query(
                vec![super::Source::Custom(src_a), super::Source::Custom(src_b)],
                vec![Platform::Linux64, Platform::NoArch],
                vec![matchspec].into_iter(),
            )
            .recursive(false)
            .await
            .unwrap();

        let mut all_records: Vec<_> = records.iter().flat_map(RepoData::iter).collect();
        all_records.sort();

        // Collect matched names.
        let matched_names: std::collections::BTreeSet<_> = all_records
            .iter()
            .map(|r| r.package_record.name.as_normalized().to_string())
            .collect();

        assert_eq!(
            matched_names,
            ["lib-bar", "lib-baz", "lib-foo", "lib-qux"]
                .into_iter()
                .map(String::from)
                .collect::<std::collections::BTreeSet<_>>(),
            "glob should find all lib-* packages across both sources and both subdirs"
        );
    }

    /// Mock source that records every name it has been asked to fetch.
    struct RecordingSource {
        inner: MockRepoDataSource,
        fetched: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl RecordingSource {
        fn new() -> (Self, Arc<std::sync::Mutex<Vec<String>>>) {
            let fetched = Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    inner: MockRepoDataSource::new(),
                    fetched: fetched.clone(),
                },
                fetched,
            )
        }

        fn add(&mut self, platform: Platform, rec: RepoDataRecord) {
            self.inner.add_record(platform, rec);
        }
    }

    #[async_trait::async_trait]
    impl super::RepoDataSource for RecordingSource {
        async fn fetch_package_records(
            &self,
            platform: Platform,
            name: &PackageName,
        ) -> Result<Vec<Arc<RepoDataRecord>>, GatewayError> {
            self.fetched
                .lock()
                .unwrap()
                .push(name.as_normalized().to_string());
            self.inner.fetch_package_records(platform, name).await
        }

        fn package_names(&self, platform: Platform) -> Vec<String> {
            self.inner.package_names(platform)
        }
    }

    /// `outer` depends on `black` (no extras). Black is reached via the
    /// transitive walk; aiohttp lives in black's [d] extra. Since [d] was
    /// never activated, aiohttp must not be fetched.
    #[tokio::test]
    async fn extras_skipped_when_inactive_transitive() {
        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        src.add(
            Platform::Linux64,
            make_test_record_full("outer", "1.0.0", "linux-64", &["black"], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "black",
                "25.0.0",
                "linux-64",
                &["click >=8"],
                &[("d", &["aiohttp >=3"])],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("click", "8.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("aiohttp", "3.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("outer", Lenient).unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(names.contains(&"outer".to_string()));
        assert!(names.contains(&"black".to_string()));
        assert!(names.contains(&"click".to_string()));
        assert!(
            !names.contains(&"aiohttp".to_string()),
            "aiohttp should not be fetched when black's [d] extra is inactive; got {names:?}",
        );
    }

    /// A transitive package can introduce an extra on another package. Here
    /// `helper` has a base dep `black[extras=[d]]`. Asking for `helper`
    /// should pull both black and aiohttp.
    #[tokio::test]
    async fn extras_walk_followed_when_active_via_transitive() {
        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        src.add(
            Platform::Linux64,
            make_test_record_full("helper", "1.0.0", "linux-64", &["black[extras=[d]]"], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "black",
                "25.0.0",
                "linux-64",
                &[],
                &[("d", &["aiohttp >=3"])],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("aiohttp", "3.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("helper", Lenient).unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(names.contains(&"helper".to_string()));
        assert!(names.contains(&"black".to_string()));
        assert!(
            names.contains(&"aiohttp".to_string()),
            "aiohttp should be fetched via the transitive [d] activation; got {names:?}",
        );
    }

    /// `helper` depends on `aiohttp` (base) and on `tornado`, and `tornado`
    /// in turn depends on `aiohttp[extras=[speedups]]`. The [speedups] extra
    /// may activate after aiohttp's records have already arrived; when that
    /// happens we must still walk it. `speedups_helper` is gated by the
    /// speedups extra and must end up fetched.
    #[tokio::test]
    async fn extras_late_activation_walks_cached_records() {
        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        src.add(
            Platform::Linux64,
            make_test_record_full("helper", "1.0.0", "linux-64", &["aiohttp", "tornado"], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "aiohttp",
                "3.0.0",
                "linux-64",
                &[],
                &[("speedups", &["speedups_helper >=1"])],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "tornado",
                "6.0.0",
                "linux-64",
                &["aiohttp[extras=[speedups]]"],
                &[],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("speedups_helper", "1.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("helper", Lenient).unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(names.contains(&"helper".to_string()));
        assert!(names.contains(&"aiohttp".to_string()));
        assert!(names.contains(&"tornado".to_string()));
        assert!(
            names.contains(&"speedups_helper".to_string()),
            "late activation of aiohttp[speedups] should walk the extra's deps; got {names:?}",
        );
    }

    /// `pkg[full]` activates `pkg[a]` and `pkg[b]`, both of which add their
    /// own deps. Chained activation on the same package must terminate and
    /// fetch all of the involved deps.
    #[tokio::test]
    async fn extras_chained_activation_same_package() {
        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        src.add(
            Platform::Linux64,
            make_test_record_full("driver", "1.0.0", "linux-64", &["pkg[extras=[full]]"], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "pkg",
                "1.0.0",
                "linux-64",
                &[],
                &[
                    ("full", &["pkg[extras=[a]]", "pkg[extras=[b]]"]),
                    ("a", &["dep_a >=1"]),
                    ("b", &["dep_b >=1"]),
                ],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("dep_a", "1.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("dep_b", "1.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("driver", Lenient).unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(
            names.contains(&"dep_a".to_string()),
            "[full] should chain to [a]; got {names:?}",
        );
        assert!(
            names.contains(&"dep_b".to_string()),
            "[full] should chain to [b]; got {names:?}",
        );
    }

    fn extras_options() -> rattler_conda_types::ParseMatchSpecOptions {
        rattler_conda_types::ParseMatchSpecOptions::default().with_extras(true)
    }

    /// User asks for `black` directly (Input). Black has extras `d` and
    /// `jupyter`, neither requested. Neither aiohttp nor ipython must be
    /// fetched.
    #[tokio::test]
    async fn extras_input_skipped_when_none_active() {
        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "black",
                "25.0.0",
                "linux-64",
                &["click >=8"],
                &[("d", &["aiohttp >=3"]), ("jupyter", &["ipython >=8"])],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("click", "8.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("aiohttp", "3.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("ipython", "8.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("black", Lenient).unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(names.contains(&"black".to_string()));
        assert!(names.contains(&"click".to_string()));
        assert!(
            !names.contains(&"aiohttp".to_string()),
            "aiohttp must not be fetched when black has no active extras; got {names:?}",
        );
        assert!(
            !names.contains(&"ipython".to_string()),
            "ipython must not be fetched when black has no active extras; got {names:?}",
        );
    }

    /// User asks for `black[extras=[d]]`. aiohttp must be fetched; the other
    /// extra's deps must not.
    #[tokio::test]
    async fn extras_input_walks_only_requested() {
        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "black",
                "25.0.0",
                "linux-64",
                &[],
                &[("d", &["aiohttp >=3"]), ("jupyter", &["ipython >=8"])],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("aiohttp", "3.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("ipython", "8.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("black[extras=[d]]", extras_options()).unwrap()]
                    .into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(names.contains(&"aiohttp".to_string()));
        assert!(
            !names.contains(&"ipython".to_string()),
            "ipython must not be fetched when only [d] is active; got {names:?}",
        );
    }

    /// Pattern-expanded specs must still merge extras when another input spec
    /// already queued the same exact package name.
    #[tokio::test]
    async fn extras_pattern_merges_with_existing_input() {
        use rattler_conda_types::ParseMatchSpecOptions;

        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "black",
                "25.0.0",
                "linux-64",
                &["click >=8"],
                &[("d", &["aiohttp >=3"])],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("click", "8.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("aiohttp", "3.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        let pattern = MatchSpec::from_str(
            "bla*[extras=[d]]",
            ParseMatchSpecOptions::strict()
                .with_exact_names_only(false)
                .with_extras(true),
        )
        .unwrap();

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("black", Lenient).unwrap(), pattern].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(
            names.contains(&"aiohttp".to_string()),
            "pattern extras should activate [d] even when black was already queued; got {names:?}",
        );
    }

    /// Pattern-expanded specs should activate extras even when the pattern is
    /// the only user input.
    #[tokio::test]
    async fn extras_pattern_only_walks_requested_extra() {
        use rattler_conda_types::ParseMatchSpecOptions;

        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "black",
                "25.0.0",
                "linux-64",
                &["click >=8"],
                &[("d", &["aiohttp >=3"]), ("jupyter", &["ipython >=8"])],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("click", "8.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("aiohttp", "3.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("ipython", "8.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        let pattern = MatchSpec::from_str(
            "bla*[extras=[d]]",
            ParseMatchSpecOptions::strict()
                .with_exact_names_only(false)
                .with_extras(true),
        )
        .unwrap();

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![pattern].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(
            names.contains(&"aiohttp".to_string()),
            "pattern extras should fetch deps from the requested [d] extra; got {names:?}",
        );
        assert!(
            !names.contains(&"ipython".to_string()),
            "pattern extras should not fetch inactive [jupyter] deps; got {names:?}",
        );
    }

    /// User asks for `pkg >=2` (Input with version constraint). A transitive
    /// dep later activates `pkg[extras=[d]]`. The extra's deps must only be
    /// walked from records that match the original version constraint.
    #[tokio::test]
    async fn extras_input_late_activation_respects_spec_filter() {
        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        // Two versions of pkg with different deps in the [d] extra.
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "pkg",
                "1.0.0",
                "linux-64",
                &[],
                &[("d", &["dep_for_v1 >=1"])],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "pkg",
                "2.0.0",
                "linux-64",
                &[],
                &[("d", &["dep_for_v2 >=1"])],
            ),
        );
        // Transitive activator: introduced via a separate top-level package.
        src.add(
            Platform::Linux64,
            make_test_record_full("activator", "1.0.0", "linux-64", &["pkg[extras=[d]]"], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("dep_for_v1", "1.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("dep_for_v2", "1.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![
                    MatchSpec::from_str("pkg >=2", Lenient).unwrap(),
                    MatchSpec::from_str("activator", Lenient).unwrap(),
                ]
                .into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(
            names.contains(&"dep_for_v2".to_string()),
            "dep from matching pkg record must be fetched; got {names:?}",
        );
        assert!(
            !names.contains(&"dep_for_v1".to_string()),
            "dep from non-matching pkg record must not be fetched; got {names:?}",
        );
    }

    /// Black has two extras `d` and `jupyter`. A transitive activation of [d]
    /// must fetch aiohttp but not ipython.
    #[tokio::test]
    async fn extras_walk_only_requested_extra_transitive() {
        let gateway = Gateway::new();
        let (mut src, fetched) = RecordingSource::new();
        src.add(
            Platform::Linux64,
            make_test_record_full("helper", "1.0.0", "linux-64", &["black[extras=[d]]"], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full(
                "black",
                "25.0.0",
                "linux-64",
                &[],
                &[("d", &["aiohttp >=3"]), ("jupyter", &["ipython >=8"])],
            ),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("aiohttp", "3.0.0", "linux-64", &[], &[]),
        );
        src.add(
            Platform::Linux64,
            make_test_record_full("ipython", "8.0.0", "linux-64", &[], &[]),
        );
        let source: Arc<dyn super::RepoDataSource> = Arc::new(src);

        gateway
            .query(
                vec![super::Source::Custom(source)],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("helper", Lenient).unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let names = fetched.lock().unwrap().clone();
        assert!(names.contains(&"aiohttp".to_string()));
        assert!(
            !names.contains(&"ipython".to_string()),
            "ipython must not be fetched when only [d] is active; got {names:?}",
        );
    }

    /// Late activation must walk cached records for a package across every
    /// source/subdir that already returned records for that package.
    #[tokio::test]
    async fn extras_late_activation_walks_cached_records_across_sources() {
        let gateway = Gateway::new();
        let (mut source_a, fetched_a) = RecordingSource::new();
        let (mut source_b, fetched_b) = RecordingSource::new();

        source_a.add(
            Platform::Linux64,
            make_test_record_full("driver", "1.0.0", "linux-64", &["pkg", "activator"], &[]),
        );
        source_a.add(
            Platform::Linux64,
            make_test_record_full(
                "pkg",
                "1.0.0",
                "linux-64",
                &[],
                &[("d", &["dep_from_source_a >=1"])],
            ),
        );
        source_a.add(
            Platform::Linux64,
            make_test_record_full("dep_from_source_a", "1.0.0", "linux-64", &[], &[]),
        );

        source_b.add(
            Platform::Linux64,
            make_test_record_full("activator", "1.0.0", "linux-64", &["pkg[extras=[d]]"], &[]),
        );
        source_b.add(
            Platform::Linux64,
            make_test_record_full(
                "pkg",
                "2.0.0",
                "linux-64",
                &[],
                &[("d", &["dep_from_source_b >=1"])],
            ),
        );
        source_b.add(
            Platform::Linux64,
            make_test_record_full("dep_from_source_b", "1.0.0", "linux-64", &[], &[]),
        );

        gateway
            .query(
                vec![
                    super::Source::Custom(Arc::new(source_a)),
                    super::Source::Custom(Arc::new(source_b)),
                ],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("driver", Lenient).unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let mut names = fetched_a.lock().unwrap().clone();
        names.extend(fetched_b.lock().unwrap().clone());
        assert!(
            names.contains(&"dep_from_source_a".to_string()),
            "late activation should walk cached pkg records from source A; got {names:?}",
        );
        assert!(
            names.contains(&"dep_from_source_b".to_string()),
            "late activation should walk cached pkg records from source B; got {names:?}",
        );
    }

    /// Repodata with CEP-42 `channel_relations` in `info`.
    fn make_repodata_with_relations(
        name: &str,
        version: &str,
        base: Option<&str>,
        overrides: Option<&str>,
    ) -> String {
        let mut relations = String::from("{");
        let mut first = true;
        if let Some(b) = base {
            relations.push_str(&format!("\"base\": \"{b}\""));
            first = false;
        }
        if let Some(o) = overrides {
            if !first {
                relations.push_str(", ");
            }
            relations.push_str(&format!("\"overrides\": \"{o}\""));
        }
        relations.push('}');
        format!(
            r#"{{
    "info": {{
        "subdir": "linux-64",
        "channel_relations": {relations}
    }},
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

    /// `Gateway::channel_relations` round-trips declared relations.
    #[tokio::test]
    async fn test_gateway_channel_relations_roundtrip() {
        let channel_dir = tempfile::tempdir().unwrap();
        let subdir_path = channel_dir.path().join("linux-64");
        std::fs::create_dir_all(&subdir_path).unwrap();

        let repodata = make_repodata_with_relations(
            "testpkg",
            "1.0.0",
            Some("../conda-forge"),
            Some("../legacy"),
        );
        std::fs::write(subdir_path.join("repodata.json"), &repodata).unwrap();

        let server = SimpleChannelServer::new(channel_dir.path()).await;
        let channel = server.channel();

        let gateway = Gateway::new();

        let relations = gateway
            .channel_relations(&channel, Platform::Linux64)
            .await
            .unwrap()
            .expect("repodata declares channel_relations");
        assert_eq!(relations.base.as_deref(), Some("../conda-forge"));
        assert_eq!(relations.overrides.as_deref(), Some("../legacy"));
    }

    /// No declared relations returns `None`, not an error.
    #[tokio::test]
    async fn test_gateway_channel_relations_absent() {
        let channel_dir = tempfile::tempdir().unwrap();
        let subdir_path = channel_dir.path().join("linux-64");
        std::fs::create_dir_all(&subdir_path).unwrap();
        std::fs::write(
            subdir_path.join("repodata.json"),
            make_repodata("testpkg", "1.0.0"),
        )
        .unwrap();

        let server = SimpleChannelServer::new(channel_dir.path()).await;
        let channel = server.channel();

        let gateway = Gateway::new();
        let relations = gateway
            .channel_relations(&channel, Platform::Linux64)
            .await
            .unwrap();
        assert!(relations.is_none());
    }

    /// A subdir the channel doesn't publish returns `None`, not an error.
    #[tokio::test]
    async fn test_gateway_channel_relations_missing_subdir() {
        let channel_dir = tempfile::tempdir().unwrap();
        let subdir_path = channel_dir.path().join("linux-64");
        std::fs::create_dir_all(&subdir_path).unwrap();
        std::fs::write(
            subdir_path.join("repodata.json"),
            make_repodata("testpkg", "1.0.0"),
        )
        .unwrap();

        let server = SimpleChannelServer::new(channel_dir.path()).await;
        let channel = server.channel();

        let gateway = Gateway::new();
        let relations = gateway
            .channel_relations(&channel, Platform::Osx64)
            .await
            .unwrap();
        assert!(relations.is_none());
    }

    // ----------------------------------------------------------------------
    // CEP-42 integration tests
    // ----------------------------------------------------------------------

    /// Write a linux-64 subdir with one package and optional relations.
    fn write_test_subdir(
        root: &std::path::Path,
        pkg: &str,
        version: &str,
        base: Option<&str>,
        overrides: Option<&str>,
    ) {
        let subdir = root.join("linux-64");
        std::fs::create_dir_all(&subdir).unwrap();
        let json = make_repodata_with_relations(pkg, version, base, overrides);
        std::fs::write(subdir.join("repodata.json"), json).unwrap();
    }

    /// Run a linux-64 query for `pkg` and return per-bucket package names.
    async fn query_channels(
        gateway: &Gateway,
        channels: Vec<Channel>,
        pkg: &str,
        mode: Option<crate::ChannelRelationsMode>,
        max_depth: Option<usize>,
    ) -> Result<Vec<Vec<String>>, crate::GatewayError> {
        let mut q = gateway
            .query(
                channels,
                vec![Platform::Linux64],
                vec![MatchSpec::from_str(pkg, Strict).unwrap()],
            )
            .recursive(false);
        if let Some(m) = mode {
            q = q.channel_relations(m);
        }
        if let Some(d) = max_depth {
            q = q.channel_relations_max_depth(d);
        }
        let result = q.execute().await?;
        Ok(result
            .repodata
            .into_iter()
            .map(|rd| {
                rd.iter()
                    .map(|r| r.package_record.name.as_normalized().to_string())
                    .collect()
            })
            .collect())
    }

    /// A declared `base` puts the referenced channel ahead of the
    /// declaring one in the final order.
    #[tokio::test]
    async fn test_cep42_base_expands_and_orders() {
        let dir = tempfile::tempdir().unwrap();
        let cf_root = dir.path().join("conda-forge");
        let bc_root = dir.path().join("bioconda");
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);
        write_test_subdir(&bc_root, "shared", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let server_url = server.url();
        let bioconda_url = server_url.join("bioconda/").unwrap();
        let bioconda = Channel::from_url(bioconda_url);

        let gateway = Gateway::new();
        let results = query_channels(&gateway, vec![bioconda], "shared", None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(!results[0].is_empty(), "conda-forge bucket non-empty");
        assert!(!results[1].is_empty(), "bioconda bucket non-empty");
    }

    /// `Disabled` ignores declared relations.
    #[tokio::test]
    async fn test_cep42_disabled_mode_ignores_relations() {
        let dir = tempfile::tempdir().unwrap();
        let cf_root = dir.path().join("conda-forge");
        let bc_root = dir.path().join("bioconda");
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);
        write_test_subdir(&bc_root, "shared", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let bioconda = Channel::from_url(server.url().join("bioconda/").unwrap());

        let gateway = Gateway::new();
        let results = query_channels(
            &gateway,
            vec![bioconda],
            "shared",
            Some(crate::ChannelRelationsMode::Disabled),
            None,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1, "no expansion in Disabled mode");
    }

    /// `max_depth = 1` follows `a -> b` but stops short of `c`.
    #[tokio::test]
    async fn test_cep42_max_depth_truncates_recursion() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let c = dir.path().join("c");
        write_test_subdir(&a, "shared", "1.0.0", Some("../b"), None);
        write_test_subdir(&b, "shared", "2.0.0", Some("../c"), None);
        write_test_subdir(&c, "shared", "3.0.0", None, None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let results = query_channels(&gateway, vec![a_ch], "shared", None, Some(1))
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
    }

    /// `Strict` surfaces a broken cycle as a `GatewayError`.
    #[tokio::test]
    async fn test_cep42_strict_mode_errors_on_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_test_subdir(&a, "shared", "1.0.0", Some("../b"), None);
        write_test_subdir(&b, "shared", "2.0.0", Some("../a"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let err = query_channels(
            &gateway,
            vec![a_ch],
            "shared",
            Some(crate::ChannelRelationsMode::Strict),
            None,
        )
        .await
        .expect_err("cycle must error in Strict mode");
        assert!(matches!(err, crate::GatewayError::ChannelRelationsError(_)));
    }

    /// Regression: the Strict-mode cycle error must include the
    /// offending edges, not just a count. The check runs incrementally
    /// during `observe`, so this also asserts the message survives the
    /// early-exit path.
    #[tokio::test]
    async fn test_cep42_strict_mode_cycle_error_includes_edges() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_test_subdir(&a, "shared", "1.0.0", Some("../b"), None);
        write_test_subdir(&b, "shared", "2.0.0", Some("../a"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let err = query_channels(
            &gateway,
            vec![a_ch],
            "shared",
            Some(crate::ChannelRelationsMode::Strict),
            None,
        )
        .await
        .expect_err("cycle must error in Strict mode");
        let crate::GatewayError::ChannelRelationsError(msg) = err else {
            panic!("expected ChannelRelationsError, got {err:?}");
        };
        assert!(msg.contains("cycle"), "message missing 'cycle': {msg}");
        assert!(
            msg.contains("/a/") && msg.contains("/b/"),
            "cycle error must name the offending channels; got: {msg}"
        );
    }

    /// `Warn` (default) tolerates a broken cycle.
    #[tokio::test]
    async fn test_cep42_warn_mode_tolerates_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_test_subdir(&a, "shared", "1.0.0", Some("../b"), None);
        write_test_subdir(&b, "shared", "2.0.0", Some("../a"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let results = query_channels(&gateway, vec![a_ch], "shared", None, None)
            .await
            .expect("Warn mode must not error on cycle");
        assert_eq!(results.len(), 2);
    }

    /// An absent discovered subdir produces an empty bucket, not an
    /// error (same as for user-supplied channels).
    #[tokio::test]
    async fn test_cep42_missing_discovered_channel_is_an_empty_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        write_test_subdir(&a, "shared", "1.0.0", Some("../b"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let results = query_channels(&gateway, vec![a_ch.clone()], "shared", None, None)
            .await
            .expect("absent subdir must not fail the query");
        assert_eq!(results.len(), 2);
        let empty_count = results.iter().filter(|r| r.is_empty()).count();
        assert_eq!(empty_count, 1);
    }

    /// CEP-42 only reorders channels; custom sources stay last.
    #[tokio::test]
    async fn test_cep42_custom_source_stays_at_the_end_after_reorder() {
        let dir = tempfile::tempdir().unwrap();
        let cf_root = dir.path().join("conda-forge");
        let bc_root = dir.path().join("bioconda");
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);
        write_test_subdir(&bc_root, "shared", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let bioconda = Channel::from_url(server.url().join("bioconda/").unwrap());

        let mut mock = MockRepoDataSource::new();
        mock.add_record(
            Platform::Linux64,
            make_test_record("shared", "9.9.9", "linux-64"),
        );
        let custom: Arc<dyn super::RepoDataSource> = Arc::new(mock);

        let gateway = Gateway::new();
        let result = gateway
            .query(
                vec![
                    super::Source::Channel(bioconda),
                    super::Source::Custom(custom),
                ],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .unwrap()
            .repodata;

        assert_eq!(result.len(), 3);
        assert!(
            result[2]
                .iter()
                .any(|r| r.package_record.version.as_str() == "9.9.9")
        );
    }

    /// Regression: when no channel declares relations, the default
    /// `Warn` mode must NOT silently push custom sources behind
    /// channels. Caller-supplied order must be preserved.
    #[tokio::test]
    async fn test_cep42_no_relations_preserves_caller_source_order() {
        let dir = tempfile::tempdir().unwrap();
        let cf_root = dir.path().join("conda-forge");
        // No relations declared.
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let conda_forge = Channel::from_url(server.url().join("conda-forge/").unwrap());

        // Custom source FIRST, then channel.
        let mut mock = MockRepoDataSource::new();
        mock.add_record(
            Platform::Linux64,
            make_test_record("shared", "9.9.9", "linux-64"),
        );
        let custom: Arc<dyn super::RepoDataSource> = Arc::new(mock);

        let gateway = Gateway::new();
        let result = gateway
            .query(
                vec![
                    super::Source::Custom(custom),
                    super::Source::Channel(conda_forge),
                ],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .unwrap()
            .repodata;

        assert_eq!(result.len(), 2);
        // First bucket should be the custom source (version 9.9.9), not the channel.
        assert!(
            result[0]
                .iter()
                .any(|r| r.package_record.version.as_str() == "9.9.9"),
            "custom source must remain at its caller-supplied position when no \
             relations are declared; got buckets {:?}",
            result
                .iter()
                .map(|b| b
                    .iter()
                    .map(|r| r.package_record.version.as_str())
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
    }

    /// In `Strict` mode any reference that isn't a valid CEP-42
    /// relative path (must start with `../`) must abort the query.
    /// Covers absolute URLs, `./foo`, plain names, leading-slash
    /// paths, and `http://`-style scheme-only strings.
    #[tokio::test]
    async fn test_cep42_strict_mode_errors_on_invalid_reference() {
        for bad in [
            "http://evil.example/channel",
            "conda-forge",
            "./foo",
            "/foo",
            "http://",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let a = dir.path().join("a");
            write_test_subdir(&a, "shared", "1.0.0", Some(bad), None);
            let server = SimpleChannelServer::new(dir.path()).await;
            let a_ch = Channel::from_url(server.url().join("a/").unwrap());

            let gateway = Gateway::new();
            let err = query_channels(
                &gateway,
                vec![a_ch],
                "shared",
                Some(crate::ChannelRelationsMode::Strict),
                None,
            )
            .await
            .expect_err(&format!(
                "invalid reference `{bad}` must error in Strict mode"
            ));
            assert!(
                matches!(err, crate::GatewayError::ChannelRelationsError(_)),
                "wrong error variant for `{bad}`: {err:?}"
            );
        }
    }

    /// `Gateway::names` follows CEP-42 relations: a query against
    /// `bioconda` (which declares `conda-forge` as base) returns names
    /// from both channels.
    #[tokio::test]
    async fn test_cep42_names_query_follows_relations() {
        let dir = tempfile::tempdir().unwrap();
        let cf_root = dir.path().join("conda-forge");
        let bc_root = dir.path().join("bioconda");
        write_test_subdir(&cf_root, "from-cf", "1.0.0", None, None);
        write_test_subdir(&bc_root, "from-bc", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let bioconda = Channel::from_url(server.url().join("bioconda/").unwrap());

        let gateway = Gateway::new();
        let output = gateway
            .names(vec![bioconda], vec![Platform::Linux64])
            .execute()
            .await
            .unwrap();
        let name_strs: std::collections::HashSet<_> = output
            .names
            .iter()
            .map(|n| n.as_normalized().to_string())
            .collect();
        assert!(name_strs.contains("from-bc"), "bioconda's package present");
        assert!(
            name_strs.contains("from-cf"),
            "conda-forge's package surfaced via CEP-42 expansion"
        );
    }

    /// In `Disabled` mode, `Gateway::names` does NOT follow relations.
    #[tokio::test]
    async fn test_cep42_names_query_disabled_mode_ignores_relations() {
        let dir = tempfile::tempdir().unwrap();
        let cf_root = dir.path().join("conda-forge");
        let bc_root = dir.path().join("bioconda");
        write_test_subdir(&cf_root, "from-cf", "1.0.0", None, None);
        write_test_subdir(&bc_root, "from-bc", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let bioconda = Channel::from_url(server.url().join("bioconda/").unwrap());

        let gateway = Gateway::new();
        let output = gateway
            .names(vec![bioconda], vec![Platform::Linux64])
            .channel_relations(crate::ChannelRelationsMode::Disabled)
            .execute()
            .await
            .unwrap();
        let name_strs: std::collections::HashSet<_> = output
            .names
            .iter()
            .map(|n| n.as_normalized().to_string())
            .collect();
        assert!(name_strs.contains("from-bc"));
        assert!(!name_strs.contains("from-cf"));
    }

    /// `Strict` mode on `NamesQuery` surfaces a cycle as a
    /// `GatewayError::ChannelRelationsError`.
    #[tokio::test]
    async fn test_cep42_names_query_strict_mode_errors_on_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_test_subdir(&a, "pkg-a", "1.0.0", Some("../b"), None);
        write_test_subdir(&b, "pkg-b", "2.0.0", Some("../a"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let err = gateway
            .names(vec![a_ch], vec![Platform::Linux64])
            .channel_relations(crate::ChannelRelationsMode::Strict)
            .execute()
            .await
            .expect_err("cycle must error in Strict mode");
        assert!(matches!(err, crate::GatewayError::ChannelRelationsError(_)));
    }

    /// In `Warn` mode a cycle in the declared relations surfaces as a
    /// `ChannelRelationsWarning::CycleBroken` on the query output.
    #[tokio::test]
    async fn test_cep42_warn_mode_cycle_surfaces_as_warning() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_test_subdir(&a, "shared", "1.0.0", Some("../b"), None);
        write_test_subdir(&b, "shared", "2.0.0", Some("../a"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![a_ch],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .expect("Warn mode must not error on cycle");

        assert!(
            output.warnings.iter().any(|w| matches!(
                w,
                crate::GatewayWarning::ChannelRelations(
                    crate::ChannelRelationsWarning::CycleBroken { .. }
                )
            )),
            "expected a CycleBroken warning; got {:?}",
            output.warnings,
        );
    }

    /// In `Warn` mode an invalid `base`/`overrides` reference (a
    /// reference that isn't a `../`-prefixed relative path) surfaces
    /// as a `ChannelRelationsWarning::InvalidReferenceSyntax`. The
    /// query still completes.
    #[tokio::test]
    async fn test_cep42_warn_mode_invalid_reference_surfaces_as_warning() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        // Absolute URL — exactly the case the CEP forbids and a
        // common attack shape (malicious metadata pointing at an
        // attacker-controlled URL).
        write_test_subdir(
            &a,
            "shared",
            "1.0.0",
            Some("https://evil.example/channel"),
            None,
        );

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![a_ch],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .expect("Warn mode must not error on a bad reference");

        assert!(
            output.warnings.iter().any(|w| matches!(
                w,
                crate::GatewayWarning::ChannelRelations(
                    crate::ChannelRelationsWarning::InvalidReferenceSyntax { .. }
                )
            )),
            "expected an InvalidReferenceSyntax warning; got {:?}",
            output.warnings,
        );
        // The query result must not contain a bucket for the
        // attacker URL — the reference is dropped, not followed.
        assert_eq!(
            output.repodata.len(),
            1,
            "invalid reference must not introduce a discovered bucket; got {:?}",
            output
                .repodata
                .iter()
                .map(RepoData::len)
                .collect::<Vec<_>>()
        );
    }

    /// In `Warn` mode a transitively discovered channel whose subdir
    /// fetch fails outright (e.g. a malformed repodata response)
    /// surfaces as a `ChannelRelationsWarning::DiscoveryFetchFailed`.
    /// A `404` for the discovered channel does NOT exercise this
    /// path: the gateway maps "subdir not present" to an empty
    /// bucket, not an error. To force a real failure we make the
    /// server return non-JSON content for the discovered subdir's
    /// `repodata.json`, which triggers a parse error in the fetch
    /// layer.
    #[tokio::test]
    async fn test_cep42_warn_mode_failed_discovery_fetch_surfaces_as_warning() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        write_test_subdir(&a, "shared", "1.0.0", Some("../b"), None);
        // Create a `b/linux-64` directory but write a corrupted
        // `repodata.json` so the fetch + parse fails.
        let b_subdir = b.join("linux-64");
        std::fs::create_dir_all(&b_subdir).unwrap();
        std::fs::write(b_subdir.join("repodata.json"), "{ not valid json").unwrap();

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![a_ch],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .expect("Warn mode must tolerate a discovery fetch failure");

        assert!(
            output.warnings.iter().any(|w| matches!(
                w,
                crate::GatewayWarning::ChannelRelations(
                    crate::ChannelRelationsWarning::DiscoveryFetchFailed { .. }
                )
            )),
            "expected a DiscoveryFetchFailed warning; got {:?}",
            output.warnings,
        );
    }

    /// When a query succeeds with no CEP-42 issues the `warnings`
    /// field is empty.
    #[tokio::test]
    async fn test_cep42_warn_mode_clean_query_produces_no_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let cf_root = dir.path().join("conda-forge");
        let bc_root = dir.path().join("bioconda");
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);
        write_test_subdir(&bc_root, "shared", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let bioconda = Channel::from_url(server.url().join("bioconda/").unwrap());

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![bioconda],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .unwrap();

        assert!(
            output.warnings.is_empty(),
            "clean query must not produce warnings; got {:?}",
            output.warnings,
        );
    }

    // ---------------------------------------------------------------
    // Review-driven coverage: max_depth, custom-source preservation,
    // self-relations, base==overrides, file:// channels.
    // ---------------------------------------------------------------

    /// `channel_relations_max_depth(0)` must behave exactly like
    /// `ChannelRelationsMode::Disabled`: no relation observation, no
    /// CEP-42 reordering.
    #[tokio::test]
    async fn test_cep42_max_depth_zero_equals_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let cf_root = dir.path().join("conda-forge");
        let bc_root = dir.path().join("bioconda");
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);
        write_test_subdir(&bc_root, "shared", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let bioconda = Channel::from_url(server.url().join("bioconda/").unwrap());

        let gateway = Gateway::new();
        let results = query_channels(&gateway, vec![bioconda], "shared", None, Some(0))
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "max_depth=0 must not expand relations");
    }

    /// `max_depth=0` with `[Custom, Channel]` must preserve the
    /// caller's order — the custom must stay first.
    #[tokio::test]
    async fn test_cep42_max_depth_zero_preserves_custom_first() {
        let dir = tempfile::tempdir().unwrap();
        let bc_root = dir.path().join("bioconda");
        let cf_root = dir.path().join("conda-forge");
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);
        write_test_subdir(&bc_root, "shared", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let bioconda = Channel::from_url(server.url().join("bioconda/").unwrap());

        let mut mock = MockRepoDataSource::new();
        mock.add_record(
            Platform::Linux64,
            make_test_record("shared", "9.9.9", "linux-64"),
        );
        let custom: Arc<dyn super::RepoDataSource> = Arc::new(mock);

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![
                    super::Source::Custom(custom),
                    super::Source::Channel(bioconda),
                ],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .channel_relations_max_depth(0)
            .execute()
            .await
            .unwrap();

        assert_eq!(output.repodata.len(), 2);
        assert!(
            output.repodata[0]
                .iter()
                .any(|r| r.package_record.version.as_str() == "9.9.9"),
            "custom must stay first when max_depth=0"
        );
    }

    /// A chain `a -> b -> c` with `max_depth=1` must surface a
    /// `MaxDepthExceeded` warning instead of silently dropping `c`.
    /// `Strict` mode must error.
    #[tokio::test]
    async fn test_cep42_max_depth_exceeded_warns_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let c = dir.path().join("c");
        write_test_subdir(&a, "shared", "1.0.0", Some("../b"), None);
        write_test_subdir(&b, "shared", "2.0.0", Some("../c"), None);
        write_test_subdir(&c, "shared", "3.0.0", None, None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();

        // Warn: surfaces as a warning; query still completes.
        let output = gateway
            .query(
                vec![a_ch.clone()],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .channel_relations_max_depth(1)
            .execute()
            .await
            .expect("Warn must tolerate depth-exceeded");
        assert!(
            output.warnings.iter().any(|w| matches!(
                w,
                crate::GatewayWarning::ChannelRelations(
                    crate::ChannelRelationsWarning::MaxDepthExceeded { .. }
                )
            )),
            "expected a MaxDepthExceeded warning; got {:?}",
            output.warnings,
        );

        // Strict: errors.
        let err = gateway
            .query(
                vec![a_ch],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .channel_relations(crate::ChannelRelationsMode::Strict)
            .channel_relations_max_depth(1)
            .execute()
            .await
            .expect_err("Strict must error on depth-exceeded");
        assert!(matches!(err, crate::GatewayError::ChannelRelationsError(_)));
    }

    /// `[Custom, Channel(bioconda)]` where bioconda has base
    /// conda-forge: custom stays at position 0; the discovered
    /// conda-forge slots next to bioconda.
    #[tokio::test]
    async fn test_cep42_custom_first_channel_with_relation_keeps_custom_first() {
        let dir = tempfile::tempdir().unwrap();
        let bc_root = dir.path().join("bioconda");
        let cf_root = dir.path().join("conda-forge");
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);
        write_test_subdir(&bc_root, "shared", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let bioconda = Channel::from_url(server.url().join("bioconda/").unwrap());

        let mut mock = MockRepoDataSource::new();
        mock.add_record(
            Platform::Linux64,
            make_test_record("shared", "9.9.9", "linux-64"),
        );
        let custom: Arc<dyn super::RepoDataSource> = Arc::new(mock);

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![
                    super::Source::Custom(custom),
                    super::Source::Channel(bioconda),
                ],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .unwrap();

        assert_eq!(output.repodata.len(), 3);
        // Custom is the caller's first source and must stay at index 0.
        assert!(
            output.repodata[0]
                .iter()
                .any(|r| r.package_record.version.as_str() == "9.9.9"),
            "custom must stay first; got versions: {:?}",
            output
                .repodata
                .iter()
                .map(|b| b
                    .iter()
                    .map(|r| r.package_record.version.as_str())
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>(),
        );
        // The conda-forge bucket (base of bioconda) must come before
        // the bioconda bucket.
        let pos_cf = output
            .repodata
            .iter()
            .position(|b| {
                b.iter()
                    .any(|r| r.package_record.version.as_str() == "1.0.0")
            })
            .expect("conda-forge bucket present");
        let pos_bc = output
            .repodata
            .iter()
            .position(|b| {
                b.iter()
                    .any(|r| r.package_record.version.as_str() == "2.0.0")
            })
            .expect("bioconda bucket present");
        assert!(pos_cf < pos_bc, "base must come before declaring channel");
    }

    /// `[Channel(other), Custom, Channel(bioconda)]` where bioconda
    /// has base conda-forge: the custom stays at its caller-specified
    /// position (index 1 among caller sources). conda-forge slots
    /// next to bioconda.
    #[tokio::test]
    async fn test_cep42_custom_in_middle_keeps_position() {
        let dir = tempfile::tempdir().unwrap();
        let other_root = dir.path().join("other");
        let cf_root = dir.path().join("conda-forge");
        let bc_root = dir.path().join("bioconda");
        write_test_subdir(&other_root, "shared", "0.1.0", None, None);
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);
        write_test_subdir(&bc_root, "shared", "2.0.0", Some("../conda-forge"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let other = Channel::from_url(server.url().join("other/").unwrap());
        let bioconda = Channel::from_url(server.url().join("bioconda/").unwrap());

        let mut mock = MockRepoDataSource::new();
        mock.add_record(
            Platform::Linux64,
            make_test_record("shared", "9.9.9", "linux-64"),
        );
        let custom: Arc<dyn super::RepoDataSource> = Arc::new(mock);

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![
                    super::Source::Channel(other),
                    super::Source::Custom(custom),
                    super::Source::Channel(bioconda),
                ],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .unwrap();

        assert_eq!(output.repodata.len(), 4);
        // Find the index of each known bucket.
        let pos_of = |v: &str| -> usize {
            output
                .repodata
                .iter()
                .position(|b| b.iter().any(|r| r.package_record.version.as_str() == v))
                .unwrap_or_else(|| panic!("bucket `{v}` missing"))
        };
        let pos_other = pos_of("0.1.0");
        let pos_custom = pos_of("9.9.9");
        let pos_cf = pos_of("1.0.0");
        let pos_bc = pos_of("2.0.0");
        // Caller order must be preserved across caller-supplied sources.
        assert!(pos_other < pos_custom, "other before custom");
        assert!(pos_custom < pos_bc, "custom before bioconda");
        // conda-forge is a base of bioconda, must come immediately
        // before it within bioconda's slot.
        assert!(pos_cf < pos_bc, "conda-forge before bioconda");
        // conda-forge is in bioconda's slot, after the custom.
        assert!(
            pos_custom < pos_cf,
            "custom must remain ahead of bioconda's slot"
        );
    }

    /// A channel declaring `base` and `overrides` resolving to the
    /// same channel is malformed per CEP-42. Warn surfaces it as a
    /// warning; Strict errors.
    #[tokio::test]
    async fn test_cep42_base_and_overrides_same_target() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let x = dir.path().join("x");
        write_test_subdir(&x, "shared", "1.0.0", None, None);
        write_test_subdir(&a, "shared", "2.0.0", Some("../x"), Some("../x"));

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![a_ch.clone()],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .expect("Warn must tolerate base==overrides");
        assert!(
            output.warnings.iter().any(|w| matches!(
                w,
                crate::GatewayWarning::ChannelRelations(
                    crate::ChannelRelationsWarning::BaseAndOverridesSameTarget { .. }
                )
            )),
            "expected BaseAndOverridesSameTarget warning; got {:?}",
            output.warnings,
        );

        let err = gateway
            .query(
                vec![a_ch],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .channel_relations(crate::ChannelRelationsMode::Strict)
            .execute()
            .await
            .expect_err("Strict must error on base==overrides");
        assert!(matches!(err, crate::GatewayError::ChannelRelationsError(_)));
    }

    /// A channel declaring itself as `base` is malformed per CEP-42.
    /// Warn surfaces it as a warning; Strict errors. The user
    /// listing the channel must NOT silence the warning.
    #[tokio::test]
    async fn test_cep42_self_relation_on_user_channel() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        write_test_subdir(&a, "shared", "1.0.0", Some("../a"), None);

        let server = SimpleChannelServer::new(dir.path()).await;
        let a_ch = Channel::from_url(server.url().join("a/").unwrap());

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![a_ch.clone()],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .expect("Warn must tolerate self-relation");
        assert!(
            output.warnings.iter().any(|w| matches!(
                w,
                crate::GatewayWarning::ChannelRelations(
                    crate::ChannelRelationsWarning::SelfRelation { .. }
                )
            )),
            "expected SelfRelation warning; got {:?}",
            output.warnings,
        );

        let err = gateway
            .query(
                vec![a_ch],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .channel_relations(crate::ChannelRelationsMode::Strict)
            .execute()
            .await
            .expect_err("Strict must error on self-relation");
        assert!(matches!(err, crate::GatewayError::ChannelRelationsError(_)));
    }

    /// CEP-42 references resolve against `file://` channel URLs too.
    #[tokio::test]
    async fn test_cep42_file_url_relation_expands() {
        let dir = tempfile::tempdir().unwrap();
        let cf_root = dir.path().join("conda-forge");
        let bc_root = dir.path().join("bioconda");
        write_test_subdir(&cf_root, "shared", "1.0.0", None, None);
        write_test_subdir(&bc_root, "shared", "2.0.0", Some("../conda-forge"), None);

        // Use a file:// channel URL.
        let bc_url = Url::from_file_path(&bc_root).unwrap();
        let bioconda = Channel::from_url(bc_url);

        let gateway = Gateway::new();
        let output = gateway
            .query(
                vec![bioconda],
                vec![Platform::Linux64],
                vec![MatchSpec::from_str("shared", Strict).unwrap()],
            )
            .recursive(false)
            .execute()
            .await
            .unwrap();
        assert_eq!(
            output.repodata.len(),
            2,
            "conda-forge should be discovered via the relative reference"
        );
    }
}
