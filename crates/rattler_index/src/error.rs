use opendal::Error as OpendalError;
use rattler_package_streaming::ExtractError;
use std::io;
use thiserror::Error;

/// Errors that can occur during repodata generation or indexing.
#[derive(Debug, Error)]
pub enum RepodataError {
    /// An error occurred during an I/O operation.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// An error occurred while serializing or deserializing JSON.
    #[error(transparent)]
    Serde(#[from] serde_json::Error),

    /// An error occurred while serializing to `MessagePack`.
    #[error(transparent)]
    MsgPack(#[from] rmp_serde::encode::Error),

    /// An error occurred while applying a patch.
    #[error("Patch error: {0}")]
    Patch(String),

    /// An error occurred during an Opendal operation.
    #[error(transparent)]
    Opendal(#[from] OpendalError),

    /// An error occurred while joining a background task.
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),

    /// Some packages could not be indexed and were skipped.
    /// The indexing completed for valid packages, but the caller should
    /// handle these failures.
    #[error("{} packages were skipped due to errors", .stats.total_skipped())]
    SkippedPackages {
        /// The full indexing statistics, including details of each skipped package.
        stats: super::IndexStats,
    },

    /// A generic error.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Errors that can occur when reading and parsing a single package file.
#[derive(Debug, Error)]
pub enum PackageReadError {
    /// An I/O error occurred while reading or parsing the package.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// Failed to extract a `.conda` archive (e.g. corrupt zip).
    #[error("failed to read conda archive: {0}")]
    CondaArchive(#[from] ExtractError),

    /// Failed to read the package file from storage.
    #[error("failed to read package from storage: {0}")]
    Storage(#[from] OpendalError),

    /// The archive type is not supported for indexing.
    #[error("unsupported archive type: {0}")]
    UnsupportedArchiveType(String),

    /// A background task panicked while processing the package.
    #[error("task panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
}
