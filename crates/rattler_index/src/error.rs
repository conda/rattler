use opendal::Error as OpendalError;
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

    /// A package has a build timestamp in the future.
    #[error(
        "package {filename} has a build timestamp ({timestamp}) in the future of the indexing time ({indexing_time}); refusing to index"
    )]
    InvalidTimestamp {
        /// The filename of the offending package.
        filename: String,
        /// The build timestamp of the package.
        timestamp: jiff::Timestamp,
        /// The time at which the indexing run started.
        indexing_time: jiff::Timestamp,
    },

    /// A generic error.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
