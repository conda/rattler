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

    /// A generic error.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
