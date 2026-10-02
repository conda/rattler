use std::{path::Path, sync::Arc};

use rattler_conda_types::{Channel, ChannelRelations, PackageName, RepodataRevisions};

use crate::{
    Reporter,
    gateway::{
        GatewayError,
        error::SubdirNotFoundError,
        subdir::{FetchedPackage, SubdirClient},
    },
    sparse::{FormatBucketSet, PackageFormatSelection, SparsePackage, SparseRepoData},
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
    /// Create a client directly from an already-loaded [`SparseRepoData`],
    /// without parsing anything from disk.
    pub fn new(sparse: Arc<SparseRepoData>) -> Self {
        Self { sparse }
    }

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
        buckets: FormatBucketSet,
        _reporter: Option<&dyn Reporter>,
    ) -> Result<FetchedPackage, GatewayError> {
        let sparse_repodata = self.sparse.clone();
        let name = name.clone();

        let load_records = move || {
            let SparsePackage { records, removed } = sparse_repodata
                .load_package_buckets(&name, buckets)
                .map_err(|err| {
                    GatewayError::IoError(
                        "failed to extract repodata records from sparse repodata".to_string(),
                        err,
                    )
                })?;
            Ok(FetchedPackage::from_buckets(buckets, records, removed))
        };

        #[cfg(target_arch = "wasm32")]
        return load_records();
        #[cfg(not(target_arch = "wasm32"))]
        simple_spawn_blocking::tokio::run_blocking_task(load_records).await
    }

    fn package_names(&self, selection: PackageFormatSelection) -> Vec<String> {
        self.sparse
            .package_names(selection)
            .map(Into::into)
            .collect()
    }

    fn repodata_revisions(&self) -> &RepodataRevisions {
        self.sparse.repodata_revisions()
    }

    fn channel_relations(&self) -> Option<&ChannelRelations> {
        self.sparse.channel_relations()
    }
}
