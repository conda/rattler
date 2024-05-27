use crate::install::link_script::PrePostLinkError;
use crate::install::unlink::UnlinkError;
use crate::install::{InstallError, TransactionError};
use crate::package_cache::PackageCacheError;
use simple_spawn_blocking::Cancelled;

/// An error returned by the installer
#[derive(Debug, thiserror::Error)]
pub enum InstallerError {
    /// Failed to determine the currently installed packages.
    #[error("failed to determine the currently installed packages")]
    FailedToDetectInstalledPackages(#[source] std::io::Error),

    /// Failed to construct a transaction
    #[error("failed to construct a transaction")]
    FailedToConstructTransaction(#[from] TransactionError),

    /// Failed to populate the cache with the package
    #[error("failed to fetch {0}")]
    FailedToFetch(String, #[source] PackageCacheError),

    /// Failed to link a certain package
    #[error("failed to link {0}")]
    LinkError(String, #[source] InstallError),

    /// Failed to unlink a certain package
    #[error("failed to unlink {0}")]
    UnlinkError(String, #[source] UnlinkError),

    /// A generic IO error occured
    #[error("{0}")]
    IoError(String, #[source] std::io::Error),

    /// Failed to run a pre-link script
    #[error("pre-processing failed")]
    PreProcessingFailed(#[source] PrePostLinkError),

    /// Failed to run a post-link script
    #[error("post-processing failed")]
    PostProcessingFailed(#[source] PrePostLinkError),

    /// The operation was cancelled
    #[error("the operation was cancelled")]
    Cancelled,
}

impl From<Cancelled> for InstallerError {
    fn from(_: Cancelled) -> Self {
        InstallerError::Cancelled
    }
}
