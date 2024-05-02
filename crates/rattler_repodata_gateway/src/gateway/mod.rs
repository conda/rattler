mod barrier_cell;
mod builder;
mod channel_config;
mod error;
mod local_subdir;
mod query;
mod remote_subdir;
mod repo_data;
mod sharded_subdir;
mod subdir;

pub use barrier_cell::BarrierCell;
pub use builder::GatewayBuilder;
pub use channel_config::{ChannelConfig, SourceConfig};
pub use error::GatewayError;
pub use query::GatewayQuery;
pub use repo_data::RepoData;

use crate::fetch::FetchRepoDataError;
use crate::Reporter;
use dashmap::{mapref::entry::Entry, DashMap};
use local_subdir::LocalSubdirClient;
use rattler_conda_types::{Channel, MatchSpec, Platform};
use reqwest_middleware::ClientWithMiddleware;
use std::{
    path::PathBuf,
    sync::{Arc, Weak},
};
use subdir::{Subdir, SubdirData};
use tokio::sync::broadcast;

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

impl Gateway {
    /// Constructs a simple gateway with the default configuration. Use [`Gateway::builder`] if you
    /// want more control over how the gateway is constructed.
    pub fn new() -> Self {
        Gateway::builder().finish()
    }

    /// Constructs a new gateway with the given client and channel configuration.
    pub fn builder() -> GatewayBuilder {
        GatewayBuilder::default()
    }

    /// Constructs a new [`GatewayQuery`] which can be used to query repodata records.
    pub fn query<AsChannel, ChannelIter, PlatformIter, PackageNameIter, IntoMatchSpec>(
        &self,
        channels: ChannelIter,
        platforms: PlatformIter,
        specs: PackageNameIter,
    ) -> GatewayQuery
    where
        AsChannel: Into<Channel>,
        ChannelIter: IntoIterator<Item = AsChannel>,
        PlatformIter: IntoIterator<Item = Platform>,
        <PlatformIter as IntoIterator>::IntoIter: Clone,
        PackageNameIter: IntoIterator<Item = IntoMatchSpec>,
        IntoMatchSpec: Into<MatchSpec>,
    {
        GatewayQuery::new(
            self.clone(),
            channels.into_iter().map(Into::into).collect(),
            platforms.into_iter().collect(),
            specs.into_iter().map(Into::into).collect(),
        )
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
    async fn get_or_create_subdir(
        &self,
        channel: &Channel,
        platform: Platform,
        reporter: Option<&dyn Reporter>,
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
                            // Explicitly drop the entry, so we don't block any other tasks.
                            drop(entry);

                            // The sender is still active, so we can wait for the subdir to be
                            // created.
                            return match sender.subscribe().recv().await {
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

        // At this point we have exclusive write access to this specific entry. All other tasks
        // will find a pending entry and will wait for the records to become available.
        //
        // Let's start by creating the subdir. If an error occurs we immediately return the error.
        // This will drop the sender and all other waiting tasks will receive an error.
        let subdir = Arc::new(self.create_subdir(channel, platform, reporter).await?);

        // Store the fetched files in the entry.
        self.subdirs.insert(
            (channel.clone(), platform),
            PendingOrFetched::Fetched(subdir.clone()),
        );

        // Send the records to all waiting tasks. We don't care if there are no receivers, so we
        // drop the error.
        let _ = sender.send(subdir.clone());

        Ok(subdir)
    }

    async fn create_subdir(
        &self,
        channel: &Channel,
        platform: Platform,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Subdir, GatewayError> {
        let url = channel.platform_url(platform);
        let subdir_data = if url.scheme() == "file" {
            if let Ok(path) = url.to_file_path() {
                LocalSubdirClient::from_directory(&path)
                    .await
                    .map(SubdirData::from_client)
            } else {
                return Err(GatewayError::UnsupportedUrl(
                    "unsupported file based url".to_string(),
                ));
            }
        } else if url.scheme() == "http" || url.scheme() == "https" {
            if url.host_str() == Some("fast.prefiks.dev")
                || url.host_str() == Some("fast.prefix.dev")
            {
                sharded_subdir::ShardedSubdir::new(
                    channel.clone(),
                    platform.to_string(),
                    self.client.clone(),
                    self.cache.clone(),
                    self.concurrent_requests_semaphore.clone(),
                    reporter,
                )
                .await
                .map(SubdirData::from_client)
            } else {
                remote_subdir::RemoteSubdirClient::new(
                    channel.clone(),
                    platform,
                    self.client.clone(),
                    self.cache.clone(),
                    self.channel_config.get(channel).clone(),
                )
                .await
                .map(SubdirData::from_client)
            }
        } else {
            return Err(GatewayError::UnsupportedUrl(format!(
                "'{}' is not a supported scheme",
                url.scheme()
            )));
        };

        match subdir_data {
            Ok(client) => Ok(Subdir::Found(client)),
            Err(GatewayError::FetchRepoDataError(FetchRepoDataError::NotFound(_)))
                if platform != Platform::NoArch =>
            {
                // If the subdir was not found and the platform is not `noarch` we assume its just
                // empty.
                Ok(Subdir::NotFound)
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

#[cfg(test)]
mod test {
    use crate::gateway::Gateway;
    use crate::utils::simple_channel_server::SimpleChannelServer;
    use rattler_conda_types::{Channel, PackageName, Platform};
    use std::path::Path;
    use std::str::FromStr;
    use std::time::Instant;
    use url::Url;

    fn local_conda_forge() -> Channel {
        Channel::from_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/channels/conda-forge"),
        )
    }

    async fn remote_conda_forge() -> SimpleChannelServer {
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
                vec![local_conda_forge()],
                vec![Platform::Linux64, Platform::Win32, Platform::NoArch],
                vec![PackageName::from_str("rubin-env").unwrap()].into_iter(),
            )
            .recursive(true)
            .await
            .unwrap();

        let total_records: usize = records.iter().map(|r| r.len()).sum();
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

        let total_records: usize = records.iter().map(|r| r.len()).sum();
        assert_eq!(total_records, 45060);
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

        let total_records: usize = records.iter().map(|r| r.len()).sum();
        assert_eq!(total_records, 84242);
    }
}
