#![deny(missing_docs)]

//! This crate provides the ability to extract a Conda package archive or specific parts of it.

use std::path::PathBuf;
use zip::result::ZipError;

use rattler_digest::{Md5Hash, Sha256Hash};

#[cfg(feature = "reqwest")]
use rattler_networking::Redact;

pub mod read;
pub mod seek;

#[cfg(feature = "reqwest")]
pub mod reqwest;

pub mod fs;
pub mod tokio;
pub mod write;

/// An error that can occur when extracting a package archive.
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum ExtractError {
    #[error("an io error occurred")]
    IoError(#[from] std::io::Error),

    #[error("could not create the destination path")]
    CouldNotCreateDestination(#[source] std::io::Error),

    #[error("invalid zip archive")]
    ZipError(#[source] zip::result::ZipError),

    #[error("a component is missing from the Conda archive")]
    MissingComponent,

    #[error("unsupported compression method")]
    UnsupportedCompressionMethod,

    #[cfg(feature = "reqwest")]
    #[error(transparent)]
    ReqwestError(::reqwest_middleware::Error),

    #[error("unsupported package archive format")]
    UnsupportedArchiveType,

    #[error("the task was cancelled")]
    Cancelled,

    #[error("could not parse archive member {0}: {1}")]
    ArchiveMemberParseError(PathBuf, #[source] std::io::Error),
}

impl From<ZipError> for ExtractError {
    fn from(value: ZipError) -> Self {
        match value {
            ZipError::Io(io) => Self::IoError(io),
            e => Self::ZipError(e),
        }
    }
}

#[cfg(feature = "reqwest")]
impl From<::reqwest_middleware::Error> for ExtractError {
    fn from(err: ::reqwest_middleware::Error) -> Self {
        ExtractError::ReqwestError(err.redact())
    }
}

/// Result struct returned by extraction functions.
#[derive(Debug)]
pub struct ExtractResult {
    /// The SHA256 hash of the extracted archive.
    pub sha256: Sha256Hash,

    /// The Md5 hash of the extracted archive.
    pub md5: Md5Hash,
}
