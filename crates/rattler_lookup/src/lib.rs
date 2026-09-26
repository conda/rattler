//! Reading and writing the *lookup index* of a conda channel: a static,
//! range-request friendly index that answers "which artifacts contain this
//! file?".
//!
//! The index of a subdir lives in `<subdir>/lookup/` next to the subdir's
//! repodata (or anywhere else, in which case the repodata points at it) and
//! consists of:
//!
//! * `manifest.json`: the only file that changes. It lists the *kinds* of
//!   lookups the index answers and its *layers*, see [`manifest`].
//! * Per layer, content-addressed Parquet files whose names contain the SHA-256
//!   of their bytes, see [`mod@format`]:
//!   * `packages-<sha256>.parquet`: the filenames of all artifacts the layer
//!     indexes, sorted. The row number of an artifact is its *package id*.
//!   * `<kind>-<sha256>.parquet` per kind: one row per distinct key, sorted,
//!     holding the ids of the artifacts it belongs to. `paths` is keyed by path,
//!     `reversed-paths` by the path with its components reversed, which turns
//!     `**/zlib.h` into a prefix scan.
//!
//! An update adds a small layer with the new artifacts and a new manifest
//! instead of rewriting the index, so everything but the manifest can be cached
//! forever. Layers are periodically merged into a new base layer. The repodata
//! points to the manifest with its `info.lookup_url` field.
//!
//! # Looking up a path
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use rattler_conda_types::Platform;
//! use rattler_lookup::PathLookup;
//! use url::Url;
//!
//! let channel = Url::parse("https://conda.anaconda.org/conda-forge/")?;
//! let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
//! let lookup = PathLookup::for_channel(
//!     &channel,
//!     &[Platform::Linux64, Platform::NoArch],
//!     client,
//! )
//! .await?;
//! for found in lookup.find("bin/python").await? {
//!     println!("{}/{}", found.subdir, found.file_name);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Looking up a pattern
//!
//! ```no_run
//! # async fn example(lookup: rattler_lookup::PathLookup) -> Result<(), Box<dyn std::error::Error>> {
//! use rattler_lookup::Query;
//!
//! let query = Query::parse("**/libssl.so.*")?;
//! let found = lookup.search(&query, 100).await?;
//! for (path, artifacts) in &found.paths {
//!     println!("{path}: {}", artifacts.len());
//! }
//! # Ok(())
//! # }
//! ```
#![deny(missing_docs)]

pub mod discover;
pub mod format;
pub mod manifest;
mod query;
mod read;
mod write;

use std::path::PathBuf;

pub use discover::discover_manifest_url;
pub use format::{Kind, WriteOptions};
pub use manifest::{FileRef, Layer, Manifest, ManifestError, PackagesRef};
pub use query::Query;
pub use read::{LookupStats, PathLookup, PathMatch, Search, SubdirPathLookup};
pub use write::{
    Entry, EntrySource, LayerBuilder, LayerFile, LayerFiles, LayerWriter, MergeEntries,
    layer_entries, merge_entries, package_names,
};

/// An error that occurred while reading or writing a lookup index.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LookupError {
    /// A local file could not be read.
    #[error("failed to read {}", .path.display())]
    Io {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// An HTTP request failed.
    #[error("failed to request {url}")]
    Http {
        /// The requested URL.
        url: Box<url::Url>,
        /// The underlying error.
        #[source]
        source: reqwest_middleware::Error,
    },

    /// An HTTP request returned an unexpected status.
    #[error("failed to request {url}: {status}")]
    HttpStatus {
        /// The requested URL.
        url: Box<url::Url>,
        /// The status the server returned.
        status: reqwest::StatusCode,
    },

    /// A file could not be read with range requests.
    #[error("failed to read {url} with range requests")]
    RangeRequests {
        /// The requested URL.
        url: Box<url::Url>,
        /// The underlying error.
        #[source]
        source: async_http_range_reader::AsyncHttpRangeReaderError,
    },

    /// Reading a byte range of a remote file failed.
    #[error("failed to read bytes {start}..{end} of {url}")]
    RemoteRead {
        /// The url that was read from.
        url: Box<url::Url>,
        /// The first byte of the range.
        start: u64,
        /// The end of the range (exclusive).
        end: u64,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A byte range outside of a file was requested, i.e. the file is corrupt.
    #[error("{url}: cannot read bytes {start}..{end} of a file of {len} bytes")]
    OutOfBounds {
        /// The url that was read from.
        url: Box<url::Url>,
        /// The first byte of the range.
        start: u64,
        /// The end of the range (exclusive).
        end: u64,
        /// The size of the file.
        len: u64,
    },

    /// A URL could only be constructed from an unsupported scheme.
    #[error("unsupported url scheme `{0}`, expected `http`, `https` or `file`")]
    UnsupportedScheme(String),

    /// A `file://` URL does not refer to a path on this system.
    #[error("`{0}` is not a valid file url")]
    InvalidFileUrl(Box<url::Url>),

    /// A URL could not be constructed.
    #[error(transparent)]
    Url(#[from] url::ParseError),

    /// A manifest could not be parsed.
    #[error("invalid manifest {url}")]
    InvalidManifest {
        /// The URL of the manifest.
        url: Box<url::Url>,
        /// The underlying error.
        #[source]
        source: ManifestError,
    },

    /// A layer file does not have the size the manifest promises.
    #[error("{url} has {actual} bytes, but the manifest expects {expected}")]
    SizeMismatch {
        /// The URL of the layer file.
        url: Box<url::Url>,
        /// The size given in the manifest.
        expected: u64,
        /// The actual size of the file.
        actual: u64,
    },

    /// A layer file is not a lookup index file.
    #[error("{url} is not a conda paths file: {reason}")]
    InvalidLayer {
        /// The URL of the layer file.
        url: Box<url::Url>,
        /// What is wrong with the file.
        reason: String,
    },

    /// The index does not have a table of the kind a lookup needs, so it cannot
    /// answer it — which is not the same as answering no results.
    #[error("this index has no `{0}` table")]
    KindUnavailable(Kind),

    /// A query is not a path or a pattern that an index can answer.
    #[error("`{pattern}` cannot be looked up: {reason}")]
    InvalidQuery {
        /// The query that was rejected.
        pattern: String,
        /// Why it cannot be answered.
        reason: String,
    },

    /// The rows handed to a [`LayerWriter`] were not sorted by path.
    #[error("paths must be pushed in ascending order, but `{previous}` is followed by `{path}`")]
    NotSorted {
        /// The path that was pushed before.
        previous: String,
        /// The path that was pushed out of order.
        path: String,
    },

    /// A path was pushed with an artifact that is not part of the layer.
    #[error("`{0}` is not one of the artifacts of the layer")]
    UnknownPackage(String),

    /// A lookup table refers to a row the packages file does not have.
    #[error("the package id {0} is not in the packages file of the layer")]
    UnknownPackageId(u32),

    /// A layer cannot index more artifacts than a package id can address.
    #[error("a layer cannot index more than {} artifacts", u32::MAX)]
    TooManyPackages,

    /// Reading or writing a Parquet file failed.
    #[error(transparent)]
    Parquet(#[from] parquet::errors::ParquetError),

    /// Building or reading an Arrow record batch failed.
    #[error(transparent)]
    Arrow(#[from] arrow_schema::ArrowError),

    /// A repodata file could not be decoded while discovering an index.
    #[error("failed to parse {url}")]
    InvalidRepodata {
        /// The URL of the repodata file.
        url: Box<url::Url>,
        /// The underlying error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// The result of an operation on a lookup index.
pub type Result<T, E = LookupError> = std::result::Result<T, E>;
