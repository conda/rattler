//! Error types for attestation discovery, retrieval and verification.

use url::Url;

/// Errors that can occur while fetching or verifying Sigstore attestations for
/// a conda package.
#[derive(Debug, thiserror::Error)]
pub enum SigstoreError {
    /// The package record does not carry an `attestations_sha256` field, so no
    /// attestations are advertised for it.
    #[error("no attestations are advertised for {0}")]
    NoAttestationsAdvertised(String),

    /// The package record does not carry a `sha256`, which is required to bind
    /// an attestation to the package without re-downloading it.
    #[error("{0} has no sha256 in its record, cannot bind attestations to it")]
    MissingPackageSha256(String),

    /// The sidecar URL could not be derived from the package URL.
    #[error("cannot derive an attestation sidecar URL from {0}")]
    InvalidSidecarUrl(Url),

    /// The HTTP request for the sidecar failed.
    #[error("failed to fetch attestation sidecar from {url}")]
    FetchSidecar {
        /// The sidecar URL.
        url: Url,
        /// The underlying error.
        #[source]
        source: reqwest_middleware::Error,
    },

    /// The server answered the sidecar request with a non-success status.
    #[error("attestation sidecar at {url} returned HTTP {status}")]
    SidecarHttpStatus {
        /// The sidecar URL.
        url: Url,
        /// The HTTP status code.
        status: reqwest::StatusCode,
    },

    /// Reading the response body failed.
    #[error("failed to read attestation sidecar from {url}")]
    ReadSidecar {
        /// The sidecar URL.
        url: Url,
        /// The underlying error.
        #[source]
        source: reqwest::Error,
    },

    /// The sidecar is larger than the configured maximum.
    #[error("attestation sidecar at {url} exceeds the maximum size of {max_size} bytes")]
    SidecarTooLarge {
        /// The sidecar URL.
        url: Url,
        /// The configured maximum size in bytes.
        max_size: u64,
    },

    /// The SHA256 of the sidecar bytes does not match the hash advertised in
    /// the repodata.
    #[error(
        "attestation sidecar at {url} does not match the advertised hash (expected {expected}, got {actual})"
    )]
    SidecarHashMismatch {
        /// The sidecar URL.
        url: Url,
        /// The hash advertised in the repodata, hex encoded.
        expected: Box<str>,
        /// The hash of the bytes that were received, hex encoded.
        actual: Box<str>,
    },

    /// The sidecar is not a non-empty JSON array of Sigstore bundles.
    #[error("attestation sidecar at {url} is malformed: {message}")]
    MalformedSidecar {
        /// The sidecar URL.
        url: Url,
        /// A description of the problem.
        message: String,
    },

    /// The Sigstore trusted root could not be loaded.
    #[error("failed to load the Sigstore trusted root: {0}")]
    TrustedRoot(String),

    /// Verification was required but no publishers are configured for the
    /// channel the package comes from.
    #[error("no attestation publishers are configured for {0}")]
    NoPublishersConfigured(Url),

    /// None of the attestations in the sidecar passed verification.
    #[error("no attestation for {package} passed verification:{}", format_reasons(.reasons))]
    VerificationFailed {
        /// The package filename.
        package: String,
        /// One reason per rejected bundle.
        reasons: Vec<String>,
    },
}

fn format_reasons(reasons: &[String]) -> String {
    reasons
        .iter()
        .map(|reason| format!("\n  - {reason}"))
        .collect()
}

/// A convenience alias for results in this crate.
pub type SigstoreResult<T> = Result<T, SigstoreError>;
