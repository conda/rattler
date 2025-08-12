use std::sync::Arc;

use rattler_conda_types::{PackageName, RepoDataRecord};

use super::GatewayError;
use crate::Reporter;
use coalesced_map::{CoalescedGetError, CoalescedMap};

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

/// Fetches and caches repodata records by package name for a specific
/// subdirectory of a channel.
pub struct SubdirData {
    /// The client to use to fetch repodata.
    client: Arc<dyn SubdirClient>,

    /// Previously fetched or currently pending records.
    records: CoalescedMap<PackageName, Arc<[RepoDataRecord]>>,
}

impl SubdirData {
    pub fn from_client<C: SubdirClient + 'static>(client: C) -> Self {
        Self {
            client: Arc::new(client),
            records: CoalescedMap::new(),
        }
    }

    pub async fn get_or_fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        let client = self.client.clone();
        let name_clone = name.clone();

        self.records
            .get_or_try_init(name.clone(), || async move {
                client
                    .fetch_package_records(&name_clone, reporter.as_deref())
                    .await
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

    pub fn package_names(&self) -> Vec<String> {
        self.client.package_names()
    }
}

/// A client that can be used to fetch repodata for a specific subdirectory.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait SubdirClient: Send + Sync {
    /// Fetches all repodata records for the package with the given name in a
    /// channel subdirectory.
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError>;

    /// Returns the names of all packages in the subdirectory.
    fn package_names(&self) -> Vec<String>;
}
