use opendal::Error as OpendalError;
use std::io;
use thiserror::Error;

/// Errors that can occur during repodata generation or indexing.
#[derive(Debug, Error)]
pub enum RepodataError {
    /// An error occurred during an I/O operation.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// An error occurred while serializing or deserializing JSON.
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// An error occurred while serializing to `MessagePack`.
    #[error("MessagePack serialization error: {0}")]
    MsgPack(#[from] rmp_serde::encode::Error),

    /// An error occurred while applying a patch.
    #[error("Patch error: {0}")]
    Patch(String),

    /// An error occurred during an Opendal operation (e.g., S3 or file access).
    #[error("Opendal error: {0}")]
    Opendal(#[from] OpendalError),

    /// An error occurred while joining a background task.
    #[error("Task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    /// A generic error (used for internal errors or anyhow conversions).
    #[error("Other error: {0}")]
    Other(#[from] anyhow::Error),
}
