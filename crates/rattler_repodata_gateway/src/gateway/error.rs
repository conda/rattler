use crate::fetch::FetchRepoDataError;
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
}
