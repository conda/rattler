#![deny(missing_docs)]

//! Read and write [CEP-37](https://github.com/conda/ceps/blob/main/cep-0037.md)
//! `conda-lock.yml` files.
//!
//! [`LockFile`] is the editable format model. [`Document`] additionally retains
//! the original YAML and source locations for validation and conversion errors.
//! Reading accepts documented historical conda-lock representations; writing
//! emits deterministic version-1 YAML without preserving comments or formatting.
//!
//! This crate does not solve environments, download artifacts, expand environment
//! variables, or read source files listed in lockfile metadata. Pixi conversions
//! are provided by `rattler_lock::conda_lock`.
//!
//! Enable the `miette` feature to render [`Error`] source labels with miette.
//!
//! # Compatibility
//!
//! Besides the CEP-37 schema, reading accepts the spellings conda-lock itself
//! writes: an omitted `version`, channels as plain strings, explicit `null` for
//! absent optional fields, `*` and bare version literals as Python constraints,
//! and a revision instead of an artifact digest for packages with a `source`.
//! Unlike conda-lock, SHA256-only conda packages are accepted, as CEP-37 allows.
//!
//! ```
//! use rattler_conda_lock::Document;
//!
//! let document = Document::parse("metadata:\n  content_hash: {}\n  channels: []\n  platforms: []\n  sources: []\npackage: []\n")?;
//! assert!(document.lock_file().to_yaml()?.starts_with("version: 1"));
//! # Ok::<(), rattler_conda_lock::Error>(())
//! ```

mod document;
mod error;
mod model;
mod parse;
mod serialize;
mod validation;

pub use document::Document;
pub use error::{Error, Label};
pub use model::{
    Channel, GitMetadata, Hashes, LockFile, Manager, Metadata, Package, PackageSource, TimeMetadata,
};
