use crate::fetch::FetchRepoDataError;
use crate::gateway::subdir::SubdirClient;
use crate::gateway::GatewayError;
use crate::sparse::SparseRepoData;
use rattler_conda_types::{Channel, PackageName, RepoDataRecord};
use std::path::Path;
use std::sync::Arc;
use tokio::task::JoinError;

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
        let sparse = match tokio::task::spawn_blocking(move || {
            SparseRepoData::new(channel, subdir, &repodata_path, None).map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    GatewayError::FetchRepoDataError(FetchRepoDataError::NotFound(err.into()))
                } else {
                    GatewayError::IoError("failed to parse repodata.json".to_string(), err)
                }
            })
        })
        .await
        .map_err(JoinError::try_into_panic)
        {
            Ok(result) => result?,
            Err(Ok(panic)) => std::panic::resume_unwind(panic),
            Err(_) => {
                return Err(GatewayError::IoError(
                    "loading of the repodata was cancelled".to_string(),
                    std::io::ErrorKind::Interrupted.into(),
                ));
            }
        };

        Ok(Self {
            sparse: Arc::new(sparse),
        })
    }

    pub async fn from_directory(subdir: &Path) -> Result<Self, GatewayError> {
        let subdir_name = subdir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Determine the channel from the directory path
        let channel_dir = subdir.parent().unwrap_or(subdir);
        let channel = Channel::from_directory(channel_dir);

        // Load the sparse repodata
        let repodata_path = subdir.join("repodata.json");
        Self::from_channel_subdir(&repodata_path, channel, &subdir_name).await
    }
}

#[async_trait::async_trait]
impl SubdirClient for LocalSubdirClient {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        let sparse_repodata = self.sparse.clone();
        let name = name.clone();
        match tokio::task::spawn_blocking(move || sparse_repodata.load_records(&name))
            .await
            .map_err(JoinError::try_into_panic)
        {
            Ok(Ok(records)) => Ok(records.into()),
            Ok(Err(err)) => Err(GatewayError::IoError(
                "failed to extract repodata records from sparse repodata".to_string(),
                err,
            )),
            Err(Ok(panic)) => std::panic::resume_unwind(panic),
            Err(Err(_)) => Err(GatewayError::IoError(
                "loading of the records was cancelled".to_string(),
                std::io::ErrorKind::Interrupted.into(),
            )),
        }
    }
}
