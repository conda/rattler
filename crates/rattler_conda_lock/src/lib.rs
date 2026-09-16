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
//! Enable the `miette` feature to render [`Error`] labels as source snippets.
//!
//! # Compatibility
//!
//! Besides the CEP-37 schema, reading accepts the spellings conda-lock itself
//! writes: an omitted `version`, channels as plain strings, explicit `null` for
//! absent optional fields, `*` and bare version literals as Python constraints,
//! and a revision instead of an artifact digest for packages with a `source`.
//! Unlike conda-lock, SHA256-only conda packages are accepted, as CEP-37 allows.
//!
//! # Comments and formatting
//!
//! Writing is canonical: packages and target platforms are sorted, maps use
//! lexical key order, and comments are not preserved.
//!
//! # Errors
//!
//! Every failure is an [`ErrorKind`] plus the locations it applies to: the model
//! path always, and the byte span in the original YAML whenever the diagnostic
//! comes from a [`Document`]. Match on the kind; the rendered message is
//! documentation, not API.
//!
//! ```
//! use rattler_conda_lock::{Document, ErrorKind};
//!
//! let source = "\
//! metadata:
//!   content_hash:
//!     linux-64: not-a-digest
//!   channels: []
//!   platforms: [linux-64]
//!   sources: []
//! package: []
//! ";
//! let error = Document::parse(source).unwrap_err();
//!
//! assert!(matches!(error.kind(), ErrorKind::InvalidDigest { length: 64 }));
//! assert_eq!(error.path().as_str(), "metadata.content_hash.linux-64");
//!
//! // The span points into the text that was read, so a caller can render the
//! // offending line itself, or let miette do it.
//! assert_eq!(&source[error.span().unwrap()], "not-a-digest");
//! assert_eq!(
//!     error.to_string(),
//!     "metadata.content_hash.linux-64: expected exactly 64 hexadecimal digits"
//! );
//! ```
//!
//! Diagnostics about two places carry both, which is what makes duplicate
//! entries reviewable instead of merely rejected:
//!
//! ```
//! use rattler_conda_lock::{Document, ErrorKind};
//!
//! let source = "\
//! metadata:
//!   content_hash:
//!     linux-64: 8b7df143d91c716ecfa5fc1730022f6b421b05cedee8fd52b1fc65a96030ad52
//!   channels: []
//!   platforms: [linux-64, linux-64]
//!   sources: []
//! package: []
//! ";
//! let error = Document::parse(source).unwrap_err();
//!
//! assert!(matches!(error.kind(), ErrorKind::DuplicatePlatform));
//! let [duplicate, first] = error.labels() else { panic!("expected two labels") };
//! assert_eq!(first.message(), Some("first declared here"));
//! assert!(duplicate.span().unwrap().start > first.span().unwrap().start);
//! ```

mod document;
mod error;
mod model;
mod parse;
mod serialize;
mod validation;

pub use document::Document;
pub use error::{Error, ErrorKind, Label, Labels, NodePath, Report, ValueKind};
pub use model::{
    Channel, GitMetadata, Hashes, LockFile, Manager, Metadata, Package, PackageIdentity,
    PackageSource, TimeMetadata,
};
