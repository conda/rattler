//! Error types for sigstore verification.

use thiserror::Error;

/// Result type for sigstore operations.
pub type SigstoreResult<T> = Result<T, SigstoreError>;

/// Errors that can occur during sigstore verification.
#[derive(Debug, Error)]
pub enum SigstoreError {
    /// Failed to fetch signatures from the signatures URL.
    #[error("Failed to fetch signatures from {url}: {message}")]
    FetchSignatures {
        /// The URL that was fetched.
        url: String,
        /// The error message.
        message: String,
    },

    /// Failed to parse the signatures JSON.
    #[error("Failed to parse signatures: {0}")]
    ParseSignatures(#[from] serde_json::Error),

    /// No signatures were found for the package.
    #[error("No signatures found for package at {0}")]
    NoSignatures(String),

    /// Failed to load the Sigstore trusted root.
    #[error("Failed to load Sigstore trusted root: {0}")]
    TrustedRoot(String),

    /// Failed to parse a signature bundle.
    #[error("Failed to parse signature bundle {index}: {message}")]
    ParseBundle {
        /// The index of the bundle in the array.
        index: usize,
        /// The error message.
        message: String,
    },

    /// Signature verification failed.
    #[error("Signature verification failed: {0}")]
    VerificationFailed(String),

    /// No valid signatures found that match the required publishers.
    #[error("No valid signatures found matching required publishers for channel {channel}")]
    NoMatchingPublisher {
        /// The channel URL.
        channel: String,
    },

    /// HTTP request failed.
    #[error("HTTP request failed: {0}")]
    Http(String),

    /// The package digest does not match the attestation subject.
    #[error("Package digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch {
        /// The expected digest from the attestation.
        expected: String,
        /// The actual digest of the package.
        actual: String,
    },
}
