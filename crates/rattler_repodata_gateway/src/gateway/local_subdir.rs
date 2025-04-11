use std::{path::Path, sync::Arc};

use rattler_conda_types::{Channel, PackageName, RepoDataRecord};

use crate::{
    gateway::{error::SubdirNotFoundError, subdir::SubdirClient, GatewayError},
    sparse::SparseRepoData,
    Reporter,
};

/// A client that can be used to fetch repodata for a specific subdirectory from
/// a local directory.
///
/// Use the [`LocalSubdirClient::from_directory`] function to create a new
/// instance of this client.
pub struct LocalSubdirClient {
    sparse: Arc<SparseRepoData>,
}

impl LocalSubdirClient {
    pub fn from_file(
        repodata_path: &Path,
        channel: Channel,
        subdir: &str,
    ) -> Result<Self, GatewayError> {
        let repodata_path = repodata_path.to_path_buf();
        let subdir = subdir.to_string();
        let sparse =
            SparseRepoData::from_file(channel.clone(), subdir.clone(), &repodata_path, None)
                .map_err(|err| {
                    if err.kind() == std::io::ErrorKind::NotFound {
                        GatewayError::SubdirNotFoundError(Box::new(SubdirNotFoundError {
                            channel: channel.clone(),
                            subdir: subdir.clone(),
                            source: err.into(),
                        }))
                    } else {
                        GatewayError::IoError("failed to parse repodata.json".to_string(), err)
                    }
                })?;

        Ok(Self {
            sparse: Arc::new(sparse),
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn from_bytes(
        bytes: bytes::Bytes,
        channel: Channel,
        subdir: &str,
    ) -> Result<Self, GatewayError> {
        let subdir = subdir.to_string();
        let sparse = SparseRepoData::from_bytes(channel.clone(), subdir.clone(), bytes, None)
            .map_err(|err| {
                GatewayError::IoError("failed to parse repodata.json".to_string(), err.into())
            })?;

        Ok(Self {
            sparse: Arc::new(sparse),
        })
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SubdirClient for LocalSubdirClient {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        _reporter: Option<&dyn Reporter>,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        let sparse_repodata = self.sparse.clone();
        let name = name.clone();

        let load_records = move || match sparse_repodata.load_records(&name) {
            Ok(records) => Ok(records.into()),
            Err(err) => Err(GatewayError::IoError(
                "failed to extract repodata records from sparse repodata".to_string(),
                err,
            )),
        };

        #[cfg(target_arch = "wasm32")]
        return load_records();
        #[cfg(not(target_arch = "wasm32"))]
        simple_spawn_blocking::tokio::run_blocking_task(load_records).await
    }

    fn package_names(&self) -> Vec<String> {
        let sparse_repodata: Arc<SparseRepoData> = self.sparse.clone();
        sparse_repodata
            .package_names()
            .map(std::convert::Into::into)
            .collect()
    }
}
