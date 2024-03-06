use thiserror::Error;

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("{0}")]
    IoError(String, #[source] std::io::Error),

    #[error("'{0}' is not a supported URI scheme")]
    UnsupportedScheme(String),
}