#![deny(missing_docs)]
//! Read and write the *lookup index* of a conda channel: a set of static,
//! content-addressed [Apache Parquet](https://parquet.apache.org/) files that
//! map the files shipped by a channel's artifacts to the artifacts containing
//! them, queried with a handful of HTTP range requests.
//!
//! The index of a subdir lives in `<subdir>/lookup/` and consists of a
//! [`Manifest`] (`manifest.json`, the only file that changes) and *layers*.
//! Every layer has a *packages file* listing the artifacts it indexes and one
//! *lookup table* per [`Kind`]: a sorted, unique key column and a list of row
//! numbers in the packages file. The sharded repodata index points to the
//! manifest with `info.lookup_url` (see [`discovery`]).
//!
//! * Reading: [`SubdirIndex`] opens a manifest and answers a [`Query`] (a
//!   path such as `include/zlib.h`, or a pattern such as `**/zlib.h` or
//!   `site-packages/polars/*`) by reading only the needed pages of the
//!   tables. Local files and in-memory buffers are supported besides HTTP.
//! * Writing: [`write_layer`] writes the files of a layer for a set of
//!   artifacts and their paths; [`bulk`] reads complete layer files back,
//!   e.g. to merge layers.

pub mod bulk;
pub mod discovery;
mod error;
mod fetch;
pub mod format;
mod index;
mod location;
pub mod manifest;
pub mod query;
pub mod source;
pub mod table;
pub mod write;

pub use error::LookupError;
pub use format::{Kind, WriteOptions};
pub use index::{Matches, SubdirIndex};
pub use location::Location;
pub use manifest::{Layer, Manifest};
pub use query::{PathPattern, Query};
pub use write::{WrittenFile, WrittenLayer, write_layer, write_layer_sorted};
