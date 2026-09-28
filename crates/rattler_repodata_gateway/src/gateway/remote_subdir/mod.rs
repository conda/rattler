use crate::gateway::subdir::{PackageRecords, SubdirClient};
use crate::{GatewayError, Reporter, sparse::PackageFormatSelection};
use rattler_conda_types::{ChannelRelations, PackageName, RepodataRevisions};

cfg_if::cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        mod wasm;
        pub use wasm::RemoteSubdirClient;
    } else {
        mod tokio;
        pub use tokio::RemoteSubdirClient;
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SubdirClient for RemoteSubdirClient {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        package_format_selection: PackageFormatSelection,
        reporter: Option<&dyn Reporter>,
    ) -> Result<PackageRecords, GatewayError> {
        self.sparse
            .fetch_package_records(name, package_format_selection, reporter)
            .await
    }

    fn applies_package_format_selection(&self) -> bool {
        self.sparse.applies_package_format_selection()
    }

    fn package_names(&self) -> Vec<String> {
        self.sparse.package_names()
    }

    fn repodata_revisions(&self) -> &RepodataRevisions {
        self.sparse.repodata_revisions()
    }

    fn channel_relations(&self) -> Option<&ChannelRelations> {
        self.sparse.channel_relations()
    }
}
