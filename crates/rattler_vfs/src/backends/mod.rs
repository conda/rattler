#[cfg(unix)]
pub mod nfs;
#[cfg(unix)]
pub mod nfs_fs;

use std::path::PathBuf;

use crate::{
    metadata::FSMetadata,
    mount::{MountBackend, MountSession},
};

pub async fn generate_mount(
    backend: MountBackend,
    metadata: Vec<FSMetadata>,
    mount_point: PathBuf,
) -> anyhow::Result<Box<dyn MountSession>> {
    backend.mount(metadata, mount_point).await
}
