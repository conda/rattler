//! The error type of this crate.

use std::sync::Arc;

use crate::Location;

/// Everything that can go wrong while reading or writing a lookup index.
#[derive(Debug, thiserror::Error)]
pub enum LookupError {
    /// An I/O error while reading or writing a file.
    #[error("i/o error at {location}")]
    Io {
        /// The file or URL that was accessed.
        location: Location,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// An HTTP request failed.
    #[error("failed to request {url}")]
    Http {
        /// The URL that was requested.
        url: url::Url,
        /// The underlying error.
        #[source]
        source: Arc<reqwest_middleware::Error>,
    },

    /// The server does not support HTTP range requests, which the lookup
    /// needs to read parts of the index.
    #[error("{url} does not support HTTP range requests")]
    RangeRequestsUnsupported {
        /// The URL that was requested.
        url: url::Url,
        /// The underlying error.
        #[source]
        source: Box<async_http_range_reader::AsyncHttpRangeReaderError>,
    },

    /// A range request failed.
    #[error("failed to read bytes {range:?} of {url}")]
    RangeRequest {
        /// The URL that was requested.
        url: url::Url,
        /// The requested byte range.
        range: std::ops::Range<u64>,
        /// The underlying error.
        #[source]
        source: Box<async_http_range_reader::AsyncHttpRangeReaderError>,
    },

    /// There is no lookup index at the given location.
    #[error("there is no lookup index at {0}")]
    NotFound(Location),

    /// The manifest could not be parsed or violates the specification.
    #[error("invalid manifest at {location}: {reason}")]
    InvalidManifest {
        /// The location of the manifest.
        location: Location,
        /// What is wrong with it.
        reason: String,
    },

    /// The manifest has a version this crate does not understand.
    #[error("the manifest at {location} has the unsupported version {version}")]
    UnsupportedManifestVersion {
        /// The location of the manifest.
        location: Location,
        /// The version it declares.
        version: u64,
    },

    /// The index does not provide the tables a query needs.
    #[error("the index at {location} has no `{kind}` tables")]
    MissingKind {
        /// The location of the manifest.
        location: Location,
        /// The kind that is missing.
        kind: crate::Kind,
    },

    /// A layer file is not what the manifest or its name promises.
    #[error("{location} is not a valid lookup file: {reason}")]
    InvalidFile {
        /// The location of the file.
        location: Location,
        /// What is wrong with it.
        reason: String,
    },

    /// A layer file has a different size than the manifest lists.
    #[error("{location} has {actual} bytes, but the manifest expects {expected}")]
    SizeMismatch {
        /// The location of the file.
        location: Location,
        /// The size given in the manifest.
        expected: u64,
        /// The actual size.
        actual: u64,
    },

    /// A layer file has a different SHA-256 than its name says.
    #[error("{location} has the SHA-256 {actual}, but its name says {expected}")]
    DigestMismatch {
        /// The location of the file.
        location: Box<Location>,
        /// The digest in the file name.
        expected: String,
        /// The digest of the content.
        actual: String,
    },

    /// A query cannot be answered by the index.
    #[error("cannot look up `{query}`: {reason}")]
    InvalidQuery {
        /// The query as given by the user.
        query: String,
        /// Why it cannot be answered.
        reason: String,
    },

    /// The data given to the writer is invalid.
    #[error("cannot write layer: {0}")]
    InvalidInput(String),

    /// An error from the Parquet reader or writer.
    #[error(transparent)]
    Parquet(#[from] parquet::errors::ParquetError),

    /// An error from Arrow.
    #[error(transparent)]
    Arrow(#[from] arrow_schema::ArrowError),

    /// A JSON (de)serialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl LookupError {
    pub(crate) fn io(location: &Location, source: std::io::Error) -> Self {
        Self::Io {
            location: location.clone(),
            source,
        }
    }

    pub(crate) fn invalid_file(location: &Location, reason: impl Into<String>) -> Self {
        Self::InvalidFile {
            location: location.clone(),
            reason: reason.into(),
        }
    }
}
