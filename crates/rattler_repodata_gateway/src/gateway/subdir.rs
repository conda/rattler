use super::GatewayError;
use crate::gateway::PendingOrFetched;
use crate::Reporter;
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use rattler_conda_types::{PackageName, RepoDataRecord};
use std::sync::Arc;
use tokio::{sync::broadcast, task::JoinError};

pub enum Subdir {
    /// The subdirectory is missing from the channel, it is considered empty.
    NotFound,

    /// A subdirectory and the data associated with it.
    Found(SubdirData),
}

impl Subdir {
    /// Returns the names of all packages in the subdirectory.
    pub fn package_names(&self) -> Option<Vec<String>> {
        match self {
            Subdir::Found(subdir) => Some(subdir.package_names()),
            Subdir::NotFound => None,
        }
    }
}

/// Fetches and caches repodata records by package name for a specific subdirectory of a channel.
pub struct SubdirData {
    /// The client to use to fetch repodata.
    client: Arc<dyn SubdirClient>,

    /// Previously fetched or currently pending records.
    records: DashMap<PackageName, PendingOrFetched<Arc<[RepoDataRecord]>>>,
}

impl SubdirData {
    pub fn from_client<C: SubdirClient + 'static>(client: C) -> Self {
        Self {
            client: Arc::new(client),
            records: DashMap::default(),
        }
    }

    pub async fn get_or_fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        let sender = match self.records.entry(name.clone()) {
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
                            return match receiver.recv().await {
                                Ok(records) => Ok(records),
                                Err(_) => {
                                    // If this happens the sender was dropped. We simply have to
                                    // retry.
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
        // Let's start by fetching the records. If an error occurs we immediately return the error.
        // This will drop the sender and all other waiting tasks will receive an error.
        let records = match tokio::spawn({
            let client = self.client.clone();
            let name = name.clone();
            async move {
                client
                    .fetch_package_records(&name, reporter.as_deref())
                    .await
            }
        })
        .await
        .map_err(JoinError::try_into_panic)
        {
            Ok(Ok(records)) => records,
            Ok(Err(err)) => return Err(err),
            Err(Ok(panic)) => std::panic::resume_unwind(panic),
            Err(Err(_)) => {
                return Err(GatewayError::IoError(
                    "fetching records was cancelled".to_string(),
                    std::io::ErrorKind::Interrupted.into(),
                ));
            }
        };

        // Store the fetched files in the entry.
        self.records
            .insert(name.clone(), PendingOrFetched::Fetched(records.clone()));

        // Send the records to all waiting tasks. We don't care if there are no receivers so we
        // drop the error.
        let _ = sender.send(records.clone());

        Ok(records)
    }

    pub fn package_names(&self) -> Vec<String> {
        self.client.package_names()
    }
}

/// A client that can be used to fetch repodata for a specific subdirectory.
#[async_trait::async_trait]
pub trait SubdirClient: Send + Sync {
    /// Fetches all repodata records for the package with the given name in a channel subdirectory.
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError>;

    /// Returns the names of all packages in the subdirectory.
    fn package_names(&self) -> Vec<String>;
}
