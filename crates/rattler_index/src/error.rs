use thiserror::Error;
use opendal::Error as OpendalError;
use serde_json::Error as SerdeJsonError;
use std::io;

#[derive(Debug, Error)]
pub enum RepodataError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] SerdeJsonError),

    #[error("Patch application failed: {0}")]
    Patch(String),

    #[error("Opendal error: {0}")]
    Opendal(#[from] OpendalError),

    #[error("Other error: {0}")]
    Other(#[from] anyhow::Error),
}
