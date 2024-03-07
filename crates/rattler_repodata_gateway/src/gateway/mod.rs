mod error;
mod local_subdir;
mod subdir;

pub use error::GatewayError;

use crate::utils::BarrierCell;
use dashmap::{mapref::entry::Entry, DashMap};
use futures::{select_biased, stream::FuturesUnordered, StreamExt};
use itertools::Itertools;
use local_subdir::LocalSubdirClient;
use rattler_conda_types::{Channel, PackageName, Platform, RepoDataRecord};
use std::{
    borrow::Borrow,
    collections::HashSet,
    sync::{Arc, Weak},
};
use subdir::Subdir;
use tokio::sync::broadcast;

// TODO: Instead of using `Channel` it would be better if we could use just the base url. Maybe we
//  can wrap that in a type. Mamba has the CondaUrl class.

#[derive(Clone)]
pub struct Gateway {
    inner: Arc<GatewayInner>,
}

impl Gateway {
    pub fn new() -> Self {
        Self {
            inner: Arc::default(),
        }
    }

    /// Recursively loads all repodata records for the given channels, platforms and package names.
    ///
    /// This function will asynchronously load the repodata from all subdirectories (combination of
    /// channels and platforms) and recursively load all repodata records and the dependencies of
    /// the those records.
    ///
    /// Most processing will happen on the background so downloading and parsing can happen
    /// simultaneously.
    ///
    /// Repodata is cached by the [`Gateway`] so calling this function twice with the same channels
    /// will not result in the repodata being fetched twice.
    pub async fn load_records_recursive<
        AsChannel,
        ChannelIter,
        PlatformIter,
        PackageNameIter,
        IntoPackageName,
    >(
        &self,
        channels: ChannelIter,
        platforms: PlatformIter,
        names: PackageNameIter,
    ) -> Result<Vec<RepoDataRecord>, GatewayError>
    where
        AsChannel: Borrow<Channel> + Clone,
        ChannelIter: IntoIterator<Item = AsChannel>,
        PlatformIter: IntoIterator<Item = Platform>,
        <PlatformIter as IntoIterator>::IntoIter: Clone,
        PackageNameIter: IntoIterator<Item = IntoPackageName>,
        IntoPackageName: Into<PackageName>,
    {
        // Collect all the channels and platforms together
        let channels_and_platforms = channels
            .into_iter()
            .cartesian_product(platforms.into_iter())
            .collect_vec();

        // Create barrier cells for each subdirectory. This can be used to wait until the subdir
        // becomes available.
        let mut subdirs = Vec::with_capacity(channels_and_platforms.len());
        let mut pending_subdirs = FuturesUnordered::new();
        for (channel, platform) in channels_and_platforms.into_iter() {
            // Create a barrier so work that need this subdir can await it.
            let barrier = Arc::new(BarrierCell::new());
            subdirs.push(barrier.clone());

            let inner = self.inner.clone();
            pending_subdirs.push(async move {
                let subdir = inner
                    .get_or_create_subdir(channel.borrow(), platform)
                    .await?;
                barrier.set(subdir).expect("subdir was set twice");
                Ok(())
            });
        }

        // Package names that we still need to fetch.
        let mut pending_package_names = names.into_iter().map(Into::into).collect_vec();

        // Package names that we have or will issue requests for.
        let mut seen = pending_package_names
            .iter()
            .cloned()
            .collect::<HashSet<_>>();

        // A list of futures to fetch the records for the pending package names. The main task
        // awaits these futures.
        let mut pending_records = FuturesUnordered::new();

        // The resulting list of repodata records.
        let mut result = Vec::new();

        // Loop until all pending package names have been fetched.
        loop {
            // Iterate over all pending package names and create futures to fetch them from all
            // subdirs.
            for pending_package_name in pending_package_names.drain(..) {
                for subdir in subdirs.iter().cloned() {
                    let pending_package_name = pending_package_name.clone();
                    pending_records.push(async move {
                        let barrier_cell = subdir.clone();
                        let subdir = barrier_cell.wait().await;
                        subdir
                            .get_or_fetch_package_records(&pending_package_name)
                            .await
                    });
                }
            }

            // Wait for the subdir to become available.
            select_biased! {
                // Handle any error that was emitted by the pending subdirs.
                subdir_result = pending_subdirs.select_next_some() => {
                    if let Err(subdir_result) = subdir_result {
                        return Err(subdir_result);
                    }
                }

                // Handle any records that were fetched
                records = pending_records.select_next_some() => {
                    let records = records?;

                    // Extract the dependencies from the records and recursively add them to the
                    // list of package names that we need to fetch.
                    for record in records.iter() {
                        for dependency in &record.package_record.depends {
                            let dependency_name = PackageName::new_unchecked(
                                dependency.split_once(' ').unwrap_or((dependency, "")).0,
                            );
                            if seen.insert(dependency_name.clone()) {
                                pending_package_names.push(dependency_name.clone());
                            }
                        }
                    }

                    // Add the records to the result
                    result.extend_from_slice(&records);
                }

                // All futures have been handled, all subdirectories have been loaded and all
                // repodata records have been fetched.
                complete => {
                    break;
                }
            }
        }

        Ok(result)
    }
}

#[derive(Default)]
struct GatewayInner {
    /// A map of subdirectories for each channel and platform.
    subdirs: DashMap<(Channel, Platform), PendingOrFetched<Arc<Subdir>>>,
}

impl GatewayInner {
    /// Returns the [`Subdir`] for the given channel and platform. This function will create the
    /// [`Subdir`] if it does not exist yet, otherwise it will return the previously created subdir.
    ///
    /// If multiple threads request the same subdir their requests will be coalesced, and they will
    /// all receive the same subdir. If an error occurs while creating the subdir all waiting tasks
    /// will also return an error.
    async fn get_or_create_subdir(
        &self,
        channel: &Channel,
        platform: Platform,
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
        let subdir = Arc::new(self.create_subdir(channel, platform).await?);

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
    ) -> Result<Subdir, GatewayError> {
        let url = channel.platform_url(platform);
        if url.scheme() == "file" {
            if let Ok(path) = url.to_file_path() {
                return Ok(Subdir::from_client(
                    LocalSubdirClient::from_directory(&path).await?,
                ));
            }
        }

        Err(GatewayError::UnsupportedScheme(url.scheme().to_string()))
    }
}

/// A record that is either pending or has been fetched.
#[derive(Clone)]
enum PendingOrFetched<T> {
    Pending(Weak<broadcast::Sender<T>>),
    Fetched(T),
}

/// A client that can be used to fetch repodata for a specific subdirectory.
#[async_trait::async_trait]
trait SubdirClient: Send + Sync {
    /// Fetches all repodata records for the package with the given name in a channel subdirectory.
    async fn fetch_package_records(
        &self,
        name: &PackageName,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError>;
}

#[cfg(test)]
mod test {
    use crate::gateway::Gateway;
    use rattler_conda_types::{Channel, PackageName, Platform};
    use std::path::Path;
    use std::str::FromStr;

    fn local_conda_forge() -> Channel {
        Channel::from_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/channels/conda-forge"),
        )
    }

    #[tokio::test]
    async fn test_gateway() {
        let gateway = Gateway::new();

        let records = gateway
            .load_records_recursive(
                vec![local_conda_forge()],
                vec![Platform::Linux64, Platform::NoArch],
                vec![PackageName::from_str("rubin-env").unwrap()].into_iter(),
            )
            .await
            .unwrap();

        assert_eq!(records.len(), 45060);
    }
}
