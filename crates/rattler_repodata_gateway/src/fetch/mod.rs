//! This module provides functionality to download and cache `repodata.json`
//! from a remote location.

use cfg_if::cfg_if;
use rattler_redaction::Redact;
use url::Url;

pub mod no_cache;

cfg_if! {
    if #[cfg(not(target_arch = "wasm32"))] {
        mod cache;
        mod with_cache;
        pub mod jlap;
        pub use with_cache::*;
    }
}

/// `RepoData` could not be found for given channel and platform
#[derive(Debug, thiserror::Error)]
pub enum RepoDataNotFoundError {
    /// There was an error on the Http request
    #[error(transparent)]
    HttpError(reqwest::Error),

    /// There was a file system error
    #[error(transparent)]
    FileSystemError(#[from] std::io::Error),
}

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum FetchRepoDataError {
    #[error("failed to acquire a lock on the repodata cache")]
    FailedToAcquireLock(#[source] anyhow::Error),

    #[error(transparent)]
    HttpError(reqwest_middleware::Error),

    #[error(transparent)]
    IoError(std::io::Error),

    #[error("failed to download {0}")]
    FailedToDownload(Url, #[source] std::io::Error),

    #[error("repodata not found")]
    NotFound(#[from] RepoDataNotFoundError),

    #[error("failed to create temporary file for repodata.json")]
    FailedToCreateTemporaryFile(#[source] std::io::Error),

    #[error("failed to persist temporary repodata.json file")]
    FailedToPersistTemporaryFile(#[from] tempfile::PersistError),

    #[error("failed to get metadata from repodata.json file")]
    FailedToGetMetadata(#[source] std::io::Error),

    #[error("failed to write cache state")]
    FailedToWriteCacheState(#[source] std::io::Error),

    #[error("there is no cache available")]
    NoCacheAvailable,

    #[error("the operation was cancelled")]
    Cancelled,
}

impl From<reqwest_middleware::Error> for FetchRepoDataError {
    fn from(err: reqwest_middleware::Error) -> Self {
        Self::HttpError(err.redact())
    }
}

impl From<reqwest::Error> for FetchRepoDataError {
    fn from(err: reqwest::Error) -> Self {
        Self::HttpError(err.redact().into())
    }
}

impl From<reqwest::Error> for RepoDataNotFoundError {
    fn from(err: reqwest::Error) -> Self {
        Self::HttpError(err.redact())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<tokio::task::JoinError> for FetchRepoDataError {
    fn from(err: tokio::task::JoinError) -> Self {
        // Rethrow any panic
        if let Ok(panic) = err.try_into_panic() {
            std::panic::resume_unwind(panic);
        }

        // Otherwise it the operation has been cancelled
        FetchRepoDataError::Cancelled
    }
}

/// Defines which type of repodata.json file to download. Usually you want to
/// use the [`Variant::AfterPatches`] variant because that reflects the repodata
/// with any patches applied.
#[derive(Default, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Fetch the `repodata.json` file. This `repodata.json` has repodata
    /// patches applied. Packages may have also been removed from this file
    /// (yanked).
    #[default]
    AfterPatches,

    /// Fetch the `repodata_from_packages.json` file. This file contains all
    /// packages with the information extracted from their index.json file.
    /// This file is not patched and contains all packages ever uploaded.
    ///
    /// Note that this file is not available for all channels. This only seems
    /// to be available for the conda-forge and bioconda channels on
    /// anaconda.org.
    FromPackages,

    /// Fetch `current_repodata.json` file. This file contains only the latest
    /// version of each package.
    ///
    /// Note that this file is not available for all channels. This only seems
    /// to be available for the conda-forge and bioconda channels on
    /// anaconda.org.
    Current,
}

impl Variant {
    /// Returns the file name of the repodata file to download.
    pub fn file_name(&self) -> &'static str {
        match self {
            Variant::AfterPatches => "repodata.json",
            Variant::FromPackages => "repodata_from_packages.json",
            Variant::Current => "current_repodata.json",
        }
    }
}

/// Defines how to use the repodata cache.
#[derive(Default, Copy, Clone, Debug, PartialEq, Eq)]
pub enum CacheAction {
    /// Use the cache if its up to date or fetch from the URL if there is no
    /// valid cached value.
    #[default]
    CacheOrFetch,

    /// Only use the cache, but error out if the cache is not up to date
    UseCacheOnly,

    /// Only use the cache, ignore whether or not it is up to date.
    ForceCacheOnly,

    /// Do not use the cache even if there is an up to date entry.
    NoCache,
}
