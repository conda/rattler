use simple_spawn_blocking::Cancelled;

use crate::{
    install::{
        clobber_registry::ClobberError, driver::PostProcessingError, link_script::PrePostLinkError,
        unlink::UnlinkError, InstallError, TransactionError,
    },
    package_cache::PackageCacheError,
};

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

    /// A generic IO error occurred
    #[error("{0}")]
    IoError(String, #[source] std::io::Error),

    /// Failed to run a pre-link script
    #[error("pre-processing failed")]
    PreProcessingFailed(#[source] PrePostLinkError),

    /// Failed to run a post-link script
    #[error("post-processing failed")]
    PostProcessingFailed(#[source] PrePostLinkError),

    /// A clobbering error occurred
    #[error("failed to unclobber clobbered files")]
    ClobberError(#[from] ClobberError),

    /// The operation was cancelled
    #[error("the operation was cancelled")]
    Cancelled,
}

impl From<Cancelled> for InstallerError {
    fn from(_: Cancelled) -> Self {
        InstallerError::Cancelled
    }
}

impl From<PostProcessingError> for InstallerError {
    fn from(value: PostProcessingError) -> Self {
        match value {
            PostProcessingError::ClobberError(err) => InstallerError::ClobberError(err),
            PostProcessingError::FailedToDetectInstalledPackages(err) => {
                InstallerError::FailedToDetectInstalledPackages(err)
            }
        }
    }
}
