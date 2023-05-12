#![deny(missing_docs)]

//! This crate provides the ability to extract a Conda package archive or specific parts of it.

use rattler_digest::{Md5Hash, Sha256Hash};

pub mod read;
pub mod seek;

#[cfg(feature = "reqwest")]
pub mod reqwest;

pub mod fs;
#[cfg(feature = "tokio")]
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
    ZipError(#[from] zip::result::ZipError),

    #[error("a component is missing from the Conda archive")]
    MissingComponent,

    #[error("unsupported compression method")]
    UnsupportedCompressionMethod,

    #[cfg(feature = "reqwest")]
    #[error(transparent)]
    ReqwestError(::reqwest::Error),

    #[error("unsupported package archive format")]
    UnsupportedArchiveType,

    #[error("the task was cancelled")]
    Cancelled,
}

/// Result struct returned by extraction functions.
#[derive(Debug)]
pub struct ExtractResult {
    /// The SHA256 hash of the extracted archive.
    pub sha256: Sha256Hash,

    /// The Md5 hash of the extracted archive.
    pub md5: Md5Hash,
}
