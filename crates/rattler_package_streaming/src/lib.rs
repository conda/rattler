#![deny(missing_docs)]

//! This crate provides the ability to extract a Conda package archive or specific parts of it.

use simple_spawn_blocking::Cancelled;
use std::path::PathBuf;
use zip::result::ZipError;

use rattler_digest::{Md5Hash, Sha256Hash};

#[cfg(feature = "reqwest")]
use rattler_redaction::Redact;

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
    #[error("an io error occurred: {0}")]
    IoError(#[from] std::io::Error),

    #[error(
        "hash mismatch when extracting {url} to {destination}: expected {expected}, got {actual}, total size {total_size} bytes"
    )]
    HashMismatch {
        url: String,
        destination: String,
        expected: String,
        actual: String,
        total_size: u64,
    },

    #[error("could not create the destination path: {0}")]
    CouldNotCreateDestination(#[source] std::io::Error),

    #[error("invalid zip archive: {0}")]
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

impl From<Cancelled> for ExtractError {
    fn from(_value: Cancelled) -> Self {
        Self::Cancelled
    }
}

impl ExtractError {
    /// Returns true if this error is transient and the operation should be retried.
    ///
    /// This checks for common transient I/O errors like broken pipes,
    /// connection resets, and unexpected EOF that can occur during
    /// network streaming operations.
    pub fn should_retry(&self) -> bool {
        match self {
            // Retry on all I/O errors during streaming - these are typically
            // transient network issues (broken pipe, connection reset, etc.)
            // The cache layer will clean up partial files on retry.
            // TODO: Add more specific checks for transient I/O errors
            ExtractError::IoError(_) => true,
            ExtractError::CouldNotCreateDestination(_) => true,
            #[cfg(feature = "reqwest")]
            ExtractError::ReqwestError(err) => {
                // Check if this is a connection error (includes broken pipe during connection)
                match err {
                    ::reqwest_middleware::Error::Reqwest(reqwest_err) => {
                        reqwest_err.is_connect() || reqwest_err.is_timeout()
                    }
                    ::reqwest_middleware::Error::Middleware(_) => false,
                }
            }
            _ => false,
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

    /// The total size of the extracted archive in bytes.
    pub total_size: u64,
}

/// A trait that can be implemented to report download progress.
pub trait DownloadReporter: Send + Sync {
    /// Called when the download starts.
    fn on_download_start(&self);
    /// Called when the download makes progress.
    fn on_download_progress(&self, bytes_downloaded: u64, total_bytes: Option<u64>);
    /// Called when the download finishes.
    fn on_download_complete(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_retry_io_error_interrupted() {
        let err = ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "interrupted",
        ));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_retry_io_error_broken_pipe() {
        let err = ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "broken pipe",
        ));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_retry_io_error_connection_reset() {
        let err = ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset",
        ));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_retry_io_error_connection_aborted() {
        let err = ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::ConnectionAborted,
            "connection aborted",
        ));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_retry_io_error_connection_refused() {
        let err = ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_retry_io_error_not_connected() {
        let err = ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "not connected",
        ));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_retry_io_error_unexpected_eof() {
        let err = ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "unexpected eof",
        ));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_retry_io_error_not_found() {
        let err = ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "not found",
        ));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_retry_io_error_permission_denied() {
        let err = ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_retry_could_not_create_destination() {
        let err =
            ExtractError::CouldNotCreateDestination(std::io::Error::other("could not create"));
        assert!(err.should_retry());
    }

    #[test]
    fn test_should_not_retry_hash_mismatch() {
        let err = ExtractError::HashMismatch {
            url: String::from("http://test.com"),
            destination: String::from("/tmp/test"),
            expected: String::from("abc123"),
            actual: String::from("def456"),
            total_size: 100,
        };
        assert!(!err.should_retry());
    }

    #[test]
    fn test_should_not_retry_zip_error() {
        let err = ExtractError::ZipError(zip::result::ZipError::InvalidArchive(
            std::borrow::Cow::Borrowed("invalid"),
        ));
        assert!(!err.should_retry());
    }

    #[test]
    fn test_should_not_retry_missing_component() {
        let err = ExtractError::MissingComponent;
        assert!(!err.should_retry());
    }

    #[test]
    fn test_should_not_retry_unsupported_compression() {
        let err = ExtractError::UnsupportedCompressionMethod;
        assert!(!err.should_retry());
    }

    #[test]
    fn test_should_not_retry_unsupported_archive_type() {
        let err = ExtractError::UnsupportedArchiveType;
        assert!(!err.should_retry());
    }

    #[test]
    fn test_should_not_retry_cancelled() {
        let err = ExtractError::Cancelled;
        assert!(!err.should_retry());
    }
}
