use std::sync::Arc;

#[derive(Clone, Debug, thiserror::Error)]
pub enum AsyncHttpRangeReaderError {
    #[error("range requests are not supported")]
    HttpRangeRequestUnsupported,

    #[error(transparent)]
    HttpError(#[from] Arc<reqwest::Error>),

    #[error("an error occurred during transport: {0}")]
    TransportError(#[source] Arc<reqwest::Error>),

    #[error("io error occurred: {0}")]
    IoError(#[source] Arc<std::io::Error>),

    #[error("content-range header is missing from response")]
    ContentRangeMissing,

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
