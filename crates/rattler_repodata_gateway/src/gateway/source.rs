//! Source enum and `RepoDataSource` trait for custom repodata providers.

use std::sync::Arc;

use rattler_conda_types::{Channel, PackageName, Platform, RepoDataRecord};

use super::{
    subdir::{extract_unique_deps, PackageRecords, SubdirClient},
    GatewayError,
};
use crate::Reporter;

/// A source of repodata records for a specific subdirectory.
///
/// Implement this trait to provide custom repodata records without
/// going through traditional channel URLs. The gateway will call
/// these methods for each platform in the query.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RepoDataSource: Send + Sync {
    /// Fetch records for a specific package name and platform.
    ///
    /// This method is called by the gateway when it needs repodata records
    /// for a particular package. The platform parameter indicates which
    /// subdirectory the gateway is querying for.
    async fn fetch_package_records(
        &self,
        platform: Platform,
        name: &PackageName,
    ) -> Result<Vec<Arc<RepoDataRecord>>, GatewayError>;

    /// Return all available package names for the given platform.
    ///
    /// This is used by the gateway to know which packages are available
    /// in this source for a given platform/subdirectory.
    fn package_names(&self, platform: Platform) -> Vec<String>;
}

/// A source of repodata, either a channel or a custom source.
///
/// This enum allows the [`Gateway::query()`](super::Gateway::query) method
/// to accept both traditional channels and custom repodata sources.
#[derive(Clone)]
pub enum Source {
    /// A traditional conda channel (expanded to all requested platforms).
    Channel(Channel),

    /// A custom repodata source (provides records for requested platforms).
    Custom(Arc<dyn RepoDataSource>),
}

impl From<Channel> for Source {
    fn from(channel: Channel) -> Self {
        Source::Channel(channel)
    }
}

impl From<Arc<dyn RepoDataSource>> for Source {
    fn from(source: Arc<dyn RepoDataSource>) -> Self {
        Source::Custom(source)
    }
}

/// Adapts a [`RepoDataSource`] to the internal [`SubdirClient`] trait
/// for a specific platform.
///
/// This adapter is used internally by the gateway to treat custom sources
/// the same way as channel subdirectories.
pub(super) struct CustomSourceClient {
    source: Arc<dyn RepoDataSource>,
    platform: Platform,
}

impl CustomSourceClient {
    /// Create a new adapter for the given source and platform.
    pub fn new(source: Arc<dyn RepoDataSource>, platform: Platform) -> Self {
        Self { source, platform }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SubdirClient for CustomSourceClient {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        _reporter: Option<&dyn Reporter>,
    ) -> Result<PackageRecords, GatewayError> {
        let records = self
            .source
            .fetch_package_records(self.platform, name)
            .await?;
        let unique_deps = extract_unique_deps(records.iter().map(|r| &**r));
        Ok(PackageRecords {
            records,
            unique_deps,
        })
    }

    fn package_names(&self) -> Vec<String> {
        self.source.package_names(self.platform)
    }
}
