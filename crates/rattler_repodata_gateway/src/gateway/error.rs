use crate::fetch::FetchRepoDataError;
use crate::utils::Cancelled;
use thiserror::Error;

#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum GatewayError {
    #[error("{0}")]
    IoError(String, #[source] std::io::Error),

    #[error(transparent)]
    FetchRepoDataError(#[from] FetchRepoDataError),

    #[error("{0}")]
    UnsupportedUrl(String),

    #[error("{0}")]
    Generic(String),

    #[error("the operation was cancelled")]
    Cancelled,
}

impl From<Cancelled> for GatewayError {
    fn from(_: Cancelled) -> Self {
        GatewayError::Cancelled
    }
}
