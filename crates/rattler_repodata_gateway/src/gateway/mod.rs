mod barrier_cell;
mod builder;
mod channel_config;
mod direct_url_query;
mod error;
mod local_subdir;
mod query;
mod remote_subdir;
mod repo_data;
mod sharded_subdir;
mod subdir;

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Weak},
};

pub use barrier_cell::BarrierCell;
pub use builder::GatewayBuilder;
pub use channel_config::{ChannelConfig, SourceConfig};
use dashmap::{mapref::entry::Entry, DashMap};
pub use error::GatewayError;
use file_url::url_to_path;
use local_subdir::LocalSubdirClient;
pub use query::{NamesQuery, RepoDataQuery};
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{Channel, MatchSpec, Platform};
pub use repo_data::RepoData;
use reqwest_middleware::ClientWithMiddleware;
use subdir::{Subdir, SubdirData};
use tokio::sync::broadcast;
use tracing::instrument;
use url::Url;

use crate::{fetch::FetchRepoDataError, gateway::error::SubdirNotFoundError, Reporter};

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
/// clonable. There is no need to wrap the gateway in an `Arc`.
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

    /// Clears any in-memory cache for the given channel.
    ///
    /// Any subsequent query will re-fetch any required data from the source.
    ///
    /// This method does not clear any on-disk cache.
    pub fn clear_repodata_cache(&self, channel: &Channel, subdirs: SubdirSelection) {
        self.inner.subdirs.retain(|key, _| {
            key.0.base_url() != channel.base_url() || !subdirs.contains(key.1.as_str())
        });
    }
}

struct GatewayInner {
    /// A map of subdirectories for each channel and platform.
    subdirs: DashMap<(Channel, Platform), PendingOrFetched<Arc<Subdir>>>,

    /// The client to use to fetch repodata.
    client: ClientWithMiddleware,

    /// The channel configuration
    channel_config: ChannelConfig,

    /// The directory to store any cache
    cache: PathBuf,

    /// The package cache, stored to reuse memory cache
    package_cache: PackageCache,

    /// A semaphore to limit the number of concurrent requests.
    concurrent_requests_semaphore: Arc<tokio::sync::Semaphore>,
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
    #[instrument(skip(self, reporter), err)]
    async fn get_or_create_subdir(
        &self,
        channel: &Channel,
        platform: Platform,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Arc<Subdir>, GatewayError> {
        let sender = match self.subdirs.entry((channel.clone(), platform)) {
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
                let subdir = entry.get();
                match subdir {
                    PendingOrFetched::Pending(sender) => {
                        let sender = sender.upgrade();

                        if let Some(sender) = sender {
                            // Create a receiver before we drop the entry. While we hold on to
                            // the entry we have exclusive access to it, this means the task
                            // currently fetching the subdir will not be able to store a value
                            // until we drop the entry.
                            // By creating the receiver here we ensure that we are subscribed
                            // before the other tasks sends a value over the channel.
                            let mut receiver = sender.subscribe();

                            // Explicitly drop the entry, so we don't block any other tasks.
                            drop(entry);

                            // The sender is still active, so we can wait for the subdir to be
                            // created.
                            return match receiver.recv().await {
                                Ok(subdir) => Ok(subdir),
                                Err(_) => {
                                    // If this happens the sender was dropped.
                                    Err(GatewayError::IoError(
                                        "a coalesced request failed".to_string(),
                                        std::io::ErrorKind::Other.into(),
                                    ))
                                }
                            };
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
                    PendingOrFetched::Fetched(records) => return Ok(records.clone()),
                }
            }
        };

        // At this point we have exclusive write access to this specific entry. All
        // other tasks will find a pending entry and will wait for the records
        // to become available.
        //
        // Let's start by creating the subdir. If an error occurs we immediately return
        // the error. This will drop the sender and all other waiting tasks will
        // receive an error.
        let subdir = Arc::new(self.create_subdir(channel, platform, reporter).await?);

        // Store the fetched files in the entry.
        self.subdirs.insert(
            (channel.clone(), platform),
            PendingOrFetched::Fetched(subdir.clone()),
        );

        // Send the records to all waiting tasks. We don't care if there are no
        // receivers, so we drop the error.
        let _ = sender.send(subdir.clone());

        Ok(subdir)
    }

    async fn create_subdir(
        &self,
        channel: &Channel,
        platform: Platform,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Subdir, GatewayError> {
        let url = channel.platform_url(platform);
        let subdir_data = if url.scheme() == "file" {
            if let Some(path) = url_to_path(&url) {
                LocalSubdirClient::from_channel_subdir(
                    &path.join("repodata.json"),
                    channel.clone(),
                    platform.as_str(),
                )
                .await
                .map(SubdirData::from_client)
            } else {
                return Err(GatewayError::UnsupportedUrl(
                    "unsupported file based url".to_string(),
                ));
            }
        } else if supports_sharded_repodata(&url) {
            sharded_subdir::ShardedSubdir::new(
                channel.clone(),
                platform.to_string(),
                self.client.clone(),
                self.cache.clone(),
                self.concurrent_requests_semaphore.clone(),
                reporter.as_deref(),
            )
            .await
            .map(SubdirData::from_client)
        } else if url.scheme() == "http"
            || url.scheme() == "https"
            || url.scheme() == "gcs"
            || url.scheme() == "oci"
        {
            remote_subdir::RemoteSubdirClient::new(
                channel.clone(),
                platform,
                self.client.clone(),
                self.cache.clone(),
                self.channel_config.get(channel).clone(),
                reporter,
            )
            .await
            .map(SubdirData::from_client)
        } else {
            return Err(GatewayError::UnsupportedUrl(format!(
                "'{}' is not a supported scheme",
                url.scheme()
            )));
        };

        match subdir_data {
            Ok(client) => Ok(Subdir::Found(client)),
            Err(GatewayError::SubdirNotFoundError(err)) if platform != Platform::NoArch => {
                // If the subdir was not found and the platform is not `noarch` we assume its
                // just empty.
                tracing::info!(
                    "subdir {} of channel {} was not found, ignoring",
                    err.subdir,
                    err.channel.canonical_name()
                );
                Ok(Subdir::NotFound)
            }
            Err(GatewayError::FetchRepoDataError(FetchRepoDataError::NotFound(err))) => {
                Err(SubdirNotFoundError {
                    subdir: platform.to_string(),
                    channel: channel.clone(),
                    source: err.into(),
                }
                .into())
            }
            Err(err) => Err(err),
        }
    }
}

/// A record that is either pending or has been fetched.
#[derive(Clone)]
enum PendingOrFetched<T> {
    Pending(Weak<broadcast::Sender<T>>),
    Fetched(T),
}

fn supports_sharded_repodata(url: &Url) -> bool {
    (url.scheme() == "http" || url.scheme() == "https")
        && (url.host_str() == Some("fast.prefiks.dev") || url.host_str() == Some("fast.prefix.dev"))
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
        fetch::CacheAction,
        gateway::Gateway,
        utils::{simple_channel_server::SimpleChannelServer, test::fetch_repo_data},
        GatewayError, RepoData, Reporter, SourceConfig, SubdirSelection,
    };

    async fn local_conda_forge() -> Channel {
        tokio::try_join!(fetch_repo_data("noarch"), fetch_repo_data("linux-64")).unwrap();
        Channel::from_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/channels/conda-forge"),
        )
    }

    async fn remote_conda_forge() -> SimpleChannelServer {
        tokio::try_join!(fetch_repo_data("noarch"), fetch_repo_data("linux-64")).unwrap();
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
                vec![MatchSpec::from_str("openssl 3.3.1 h2466b09_1", Strict).unwrap()].into_iter(),
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
                    MatchSpec::from_str("mamba 0.9.2 py39h951de11_0", Strict).unwrap(),
                    MatchSpec::from_str(openssl_url, Strict).unwrap(),
                ]
                .into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let total_records_single_openssl: usize = records.iter().map(RepoData::len).sum();
        assert_eq!(total_records_single_openssl, 4644);

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
                vec![MatchSpec::from_str("mamba 0.9.2 py39h951de11_0", Strict).unwrap()]
                    .into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(RepoData::len).sum();

        // The total number of records should be greater than the number of records
        // fetched when selecting the openssl with a direct url.
        assert!(total_records > total_records_single_openssl);
        assert_eq!(total_records, 4692);

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

        assert_matches!(gateway_error, GatewayError::MatchSpecWithoutName(_));
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
        impl Reporter for Arc<Downloads> {
            fn on_download_complete(&self, url: &Url, _index: usize) {
                self.urls.insert(url.clone());
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

        // Construct a simpel query
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
        gateway.clear_repodata_cache(&local_channel.channel(), SubdirSelection::default());
        query.clone().execute().await.unwrap();
        assert!(
            !downloads.urls.is_empty(),
            "after clearing the cache there should be new urls fetched"
        );
    }
}
