use super::GatewayError;
use crate::gateway::{PendingOrFetched, SubdirClient};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use rattler_conda_types::{PackageName, RepoDataRecord};
use std::sync::Arc;
use tokio::{sync::broadcast, task::JoinError};

/// Represents a subdirectory of a repodata directory.
pub struct Subdir {
    /// The client to use to fetch repodata.
    client: Arc<dyn SubdirClient>,

    /// Previously fetched or currently pending records.
    records: DashMap<PackageName, PendingOrFetched<Arc<[RepoDataRecord]>>>,
}

impl Subdir {
    pub fn from_client<C: SubdirClient + 'static>(client: C) -> Self {
        Self {
            client: Arc::new(client),
            records: Default::default(),
        }
    }

    pub async fn get_or_fetch_package_records(
        &self,
        name: &PackageName,
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
                            // Explicitly drop the entry, so we don't block any other tasks.
                            drop(entry);

                            // The sender is still active, so we can wait for the records to be
                            // fetched.
                            return match sender.subscribe().recv().await {
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
            async move { client.fetch_package_records(&name).await }
        })
        .await
        .map_err(JoinError::try_into_panic)
        {
            Ok(Ok(records)) => records,
            Ok(Err(err)) => return Err(GatewayError::from(err)),
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
}
