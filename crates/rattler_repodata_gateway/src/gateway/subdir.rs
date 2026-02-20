use std::sync::Arc;

use rattler_conda_types::{PackageName, RepoDataRecord};

use super::GatewayError;
use crate::Reporter;
use coalesced_map::{CoalescedGetError, CoalescedMap};

/// Records for a single package, with precomputed unique dependency strings.
///
/// The `unique_deps` field contains the deduplicated set of dependency strings
/// across all versions of the package. This avoids iterating all records
/// during dependency resolution (e.g. 2000 numpy versions × 10 deps = 20,000
/// strings reduced to ~50 unique ones).
#[derive(Clone, Debug, Default)]
pub struct PackageRecords {
    /// All repodata records for this package.
    pub records: Vec<Arc<RepoDataRecord>>,

    /// Unique dependency strings across all records.
    pub unique_deps: Arc<[String]>,
}

/// Extract the unique dependency strings from a set of records.
pub(crate) fn extract_unique_deps<'a>(
    records: impl IntoIterator<Item = &'a RepoDataRecord>,
) -> Arc<[String]> {
    let mut seen = ahash::HashSet::<String>::default();
    let mut deps = Vec::new();
    for record in records {
        for dep in &record.package_record.depends {
            if seen.insert(dep.clone()) {
                deps.push(dep.clone());
            }
        }
        for (_, extra_deps) in record.package_record.experimental_extra_depends.iter() {
            for dep in extra_deps {
                if seen.insert(dep.clone()) {
                    deps.push(dep.clone());
                }
            }
        }
    }
    Arc::from(deps)
}

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

    /// Previously fetched or currently pending records (with precomputed deps).
    records: CoalescedMap<PackageName, PackageRecords>,
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
    ) -> Result<PackageRecords, GatewayError> {
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
    ) -> Result<PackageRecords, GatewayError>;

    /// Returns the names of all packages in the subdirectory.
    fn package_names(&self) -> Vec<String>;
}
