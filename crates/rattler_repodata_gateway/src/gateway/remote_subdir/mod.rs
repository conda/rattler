use crate::gateway::subdir::SubdirClient;
use crate::{GatewayError, Reporter};
use rattler_conda_types::{PackageName, RepoDataRecord};
use std::sync::Arc;

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
        reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        self.sparse.fetch_package_records(name, reporter).await
    }

    fn package_names(&self) -> Vec<String> {
        self.sparse.package_names()
    }
}
