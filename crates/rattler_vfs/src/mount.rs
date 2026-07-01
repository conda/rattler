use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc};

#[cfg(unix)]
use crate::backends::nfs::NfsProvider;
use crate::{metadata::FSMetadata, virtual_fs_core::VirtualFSCore};

pub trait MountSession: Send + Sync {
    fn unmount(self: Box<Self>) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Copy)]
pub enum MountBackend {
    Nfs,
}

impl MountBackend {
    pub async fn mount(
        &self,
        metadata: Vec<FSMetadata>,
        mount_point: PathBuf,
    ) -> anyhow::Result<Box<dyn MountSession>> {
        match self {
            #[cfg(unix)]
            MountBackend::Nfs => {
                let fs = Arc::new(VirtualFSCore::new(metadata, mount_point.clone()));
                NfsProvider::mount(fs, mount_point).await
            }
            /// A windows machine would not be able to setup the server part of this implementation
            /// When it would be mounting to a separate server this could work with nfs on windows
            #[cfg(not(unix))]
            MountBackend::Nfs => {
                anyhow::bail!("NFS backend is not supported on this platform")
            }
        }
    }
}

/// Picks the backend to mount for, because NFS should work on most systems this is chosen as the default.
impl From<&str> for MountBackend {
    fn from(_value: &str) -> Self {
        MountBackend::Nfs
    }
}

#[async_trait]
pub trait MountProvider {
    async fn mount(
        fs: Arc<VirtualFSCore>,
        mount_point: PathBuf,
    ) -> anyhow::Result<Box<dyn MountSession>>;
}
