use simple_spawn_blocking::Cancelled;
use std::{collections::HashMap, path::PathBuf};

use crate::{
    install::{
        clobber_registry::{ClobberError, ClobberedPath},
        driver::PostProcessingError,
        link_script::PrePostLinkError,
        unlink::UnlinkError,
        InstallError, TransactionError,
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

    /// Clobbering was detected and the clobber mode is set to error.
    #[error("{} file(s) are provided by multiple packages", .0.len())]
    ClobberingDetected(HashMap<PathBuf, ClobberedPath>),

    /// The operation was cancelled
    #[error("the operation was cancelled")]
    Cancelled,

    /// Failed to create the prefix
    #[error("failed to create the prefix")]
    FailedToCreatePrefix(PathBuf, #[source] std::io::Error),

    /// Attempted to install platform-specific packages when target platform is noarch
    #[error("cannot install platform-specific packages with noarch as the target platform. The following packages have non-noarch subdirs: {}", .0.join(", "))]
    PlatformSpecificPackagesWithNoarchPlatform(Vec<String>),

    /// Failed to acquire the global cache lock
    #[error("failed to acquire global cache lock")]
    FailedToAcquireCacheLock(#[source] PackageCacheError),
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
            PostProcessingError::ClobberingDetected(paths) => {
                InstallerError::ClobberingDetected(paths)
            }
        }
    }
}
