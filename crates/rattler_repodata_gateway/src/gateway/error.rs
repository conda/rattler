use std::{
    fmt::{Display, Formatter},
    io,
};

use rattler_conda_types::{Channel, InvalidPackageNameError, MatchSpec};
use rattler_redaction::Redact;
use thiserror::Error;

use crate::{
    fetch,
    fetch::{FetchRepoDataError, RepoDataNotFoundError},
};

#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum GatewayError {
    #[error("{0}")]
    IoError(String, #[source] std::io::Error),

    #[error(transparent)]
    ReqwestError(reqwest::Error),

    #[error(transparent)]
    ReqwestMiddlewareError(anyhow::Error),

    #[error(transparent)]
    FetchRepoDataError(#[from] FetchRepoDataError),

    #[error("{0}")]
    UnsupportedUrl(String),

    #[error("{0}")]
    Generic(String),

    #[error(transparent)]
    SubdirNotFoundError(#[from] Box<SubdirNotFoundError>),

    #[error("the operation was cancelled")]
    Cancelled,

    #[cfg(not(target_arch = "wasm32"))]
    #[error("the direct url query failed for {0}")]
    DirectUrlQueryError(
        String,
        #[source] super::direct_url_query::DirectUrlQueryError,
    ),

    #[error("the match spec '{0}' does not specify a name")]
    MatchSpecWithoutName(Box<MatchSpec>),

    #[error("the package from url '{0}', doesn't have the same name as the match spec filename intents '{1}'")]
    UrlRecordNameMismatch(String, String),

    #[error(transparent)]
    InvalidPackageName(#[from] InvalidPackageNameError),

    #[error("{0}")]
    CacheError(String),

    #[error("direct url queries are not supported ({0})")]
    DirectUrlQueryNotSupported(String),
}

#[cfg(not(target_arch = "wasm32"))]
impl From<simple_spawn_blocking::Cancelled> for GatewayError {
    fn from(_: simple_spawn_blocking::Cancelled) -> Self {
        GatewayError::Cancelled
    }
}

impl From<reqwest_middleware::Error> for GatewayError {
    fn from(value: reqwest_middleware::Error) -> Self {
        match value {
            reqwest_middleware::Error::Reqwest(err) => err.into(),
            reqwest_middleware::Error::Middleware(err) => GatewayError::ReqwestMiddlewareError(err),
        }
    }
}

impl From<reqwest::Error> for GatewayError {
    fn from(value: reqwest::Error) -> Self {
        GatewayError::ReqwestError(value.redact())
    }
}

#[derive(Debug, Error)]
pub enum HttpOrFilesystemError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Filesystem(#[from] io::Error),
}

impl From<fetch::RepoDataNotFoundError> for HttpOrFilesystemError {
    fn from(value: RepoDataNotFoundError) -> Self {
        match value {
            RepoDataNotFoundError::HttpError(err) => HttpOrFilesystemError::Http(err),
            RepoDataNotFoundError::FileSystemError(err) => HttpOrFilesystemError::Filesystem(err),
        }
    }
}

/// An error that is raised when a subdirectory of a repository is not found.
#[derive(Debug, Error)]
pub struct SubdirNotFoundError {
    /// The name of the subdirectory that was not found.
    pub subdir: String,

    /// The channel that was searched.
    pub channel: Channel,

    /// The error that caused the subdirectory to not be found.
    #[source]
    pub source: HttpOrFilesystemError,
}

impl Display for SubdirNotFoundError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not find subdir '{}' in channel '{}'",
            self.subdir,
            self.channel.canonical_name()
        )
    }
}
