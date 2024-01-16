//! Error type used for [`crate::AsyncHttpRangeReader`]
use std::sync::Arc;

/// Error type used for [`crate::AsyncHttpRangeReader`]
#[derive(Clone, Debug, thiserror::Error)]
pub enum AsyncHttpRangeReaderError {
    /// The server does not support range requests
    #[error("range requests are not supported")]
    HttpRangeRequestUnsupported,

    /// Other HTTP error
    #[error(transparent)]
    HttpError(#[from] Arc<reqwest::Error>),

    /// An error occurred during transport
    #[error("an error occurred during transport: {0}")]
    TransportError(#[source] Arc<reqwest::Error>),

    /// An IO error occurred
    #[error("io error occurred: {0}")]
    IoError(#[source] Arc<std::io::Error>),

    /// Content-Range header is missing from response
    #[error("content-range header is missing from response")]
    ContentRangeMissing,

    /// Content-Length header is missing from response
    #[error("content-length header is missing from response")]
    ContentLengthMissing,

    /// Memory mapping the file failed
    #[error("memory mapping the file failed")]
    MemoryMapError(#[source] Arc<std::io::Error>),
}

impl From<std::io::Error> for AsyncHttpRangeReaderError {
    fn from(err: std::io::Error) -> Self {
        AsyncHttpRangeReaderError::IoError(Arc::new(err))
    }
}

impl From<reqwest::Error> for AsyncHttpRangeReaderError {
    fn from(err: reqwest::Error) -> Self {
        AsyncHttpRangeReaderError::TransportError(Arc::new(err))
    }
}
