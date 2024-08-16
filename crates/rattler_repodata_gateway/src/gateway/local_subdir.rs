use crate::gateway::error::SubdirNotFoundError;
use crate::gateway::subdir::SubdirClient;
use crate::gateway::GatewayError;
use crate::sparse::SparseRepoData;
use crate::Reporter;
use rattler_conda_types::{Channel, PackageName, RepoDataRecord};
use simple_spawn_blocking::tokio::run_blocking_task;
use std::path::Path;
use std::sync::Arc;

/// A client that can be used to fetch repodata for a specific subdirectory from a local directory.
///
/// Use the [`LocalSubdirClient::from_directory`] function to create a new instance of this client.
pub struct LocalSubdirClient {
    sparse: Arc<SparseRepoData>,
}

impl LocalSubdirClient {
    pub async fn from_channel_subdir(
        repodata_path: &Path,
        channel: Channel,
        subdir: &str,
    ) -> Result<Self, GatewayError> {
        let repodata_path = repodata_path.to_path_buf();
        let subdir = subdir.to_string();
        let sparse = run_blocking_task(move || {
            SparseRepoData::new(channel.clone(), subdir.clone(), &repodata_path, None).map_err(
                |err| {
                    if err.kind() == std::io::ErrorKind::NotFound {
                        GatewayError::SubdirNotFoundError(SubdirNotFoundError {
                            channel: channel.clone(),
                            subdir: subdir.clone(),
                            source: err.into(),
                        })
                    } else {
                        GatewayError::IoError("failed to parse repodata.json".to_string(), err)
                    }
                },
            )
        })
        .await?;

        Ok(Self {
            sparse: Arc::new(sparse),
        })
    }
}

#[async_trait::async_trait]
impl SubdirClient for LocalSubdirClient {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        _reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        let sparse_repodata = self.sparse.clone();
        let name = name.clone();
        run_blocking_task(move || match sparse_repodata.load_records(&name) {
            Ok(records) => Ok(records.into()),
            Err(err) => Err(GatewayError::IoError(
                "failed to extract repodata records from sparse repodata".to_string(),
                err,
            )),
        })
        .await
    }

    fn package_names(&self) -> Vec<String> {
        let sparse_repodata: Arc<SparseRepoData> = self.sparse.clone();
        sparse_repodata
            .package_names()
            .map(std::convert::Into::into)
            .collect()
    }
}
