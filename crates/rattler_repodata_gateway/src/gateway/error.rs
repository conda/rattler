use crate::fetch::FetchRepoDataError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("{0}")]
    IoError(String, #[source] std::io::Error),

    #[error(transparent)]
    FetchRepoDataError(#[from] FetchRepoDataError),

    #[error("{0}")]
    UnsupportedUrl(String),
}
