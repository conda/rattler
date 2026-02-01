//! Content Addressable Store (CAS) for deduplicating file contents across
//! packages.
//!
//! This module provides a CAS-based approach to package extraction where:
//! - Regular file contents are stored in a content-addressed store (by SHA-256
//!   hash)
//! - Files in the destination directory are hardlinked from the CAS
//! - Identical files across multiple packages share storage in the CAS
//!
//! # Architecture
//!
//! The CAS stores files in a directory structure based on their SHA-256 hash:
//! ```text
//! <cas_root>/
//!   <first 2 hex chars>/
//!     <next 2 hex chars>/
//!       <remaining hex chars>
//! ```
//!
//! For example, a file with hash `abc123...` would be stored at:
//! ```text
//! <cas_root>/ab/c1/23...
//! ```
//!
//! # Components
//!
//! - [`SyncWriter`]: A synchronous writer for streaming content to the CAS
//! - [`Writer`]: An async writer for streaming content to the CAS
//! - [`write_sync`]: Writes content from a reader to the CAS (sync)
//!
//! For tar archive extraction with CAS support, see the `rattler_cas_tar` crate.
//!
//! # Usage
//!
//! ```rust,no_run
//! use rattler_package_streaming::cas::SyncWriter;
//! use std::io::Write;
//! use std::path::Path;
//!
//! // Write content to the CAS
//! let cas_root = Path::new("/path/to/cas");
//! let mut w = SyncWriter::create(cas_root, Some("example.txt".to_string())).unwrap();
//! w.write_all(b"Hello, world!").unwrap();
//! let (hash, path) = w.finish().unwrap();
//!
//! // The file is now stored at `path` and can be hardlinked elsewhere
//! println!("Stored at {:?} with hash {:x}", path, hash);
//! ```
//!
//! # Deduplication
//!
//! When the same content is written multiple times, the CAS automatically
//! deduplicates:
//! - The hash is computed during writing
//! - If a file with the same hash already exists, the new write is discarded
//! - The existing file path is returned
//!
//! This means extracting multiple packages with identical files only stores
//! each unique file once.

pub use rattler_cas::{write_sync, SyncWriter, Writer};
