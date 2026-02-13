#![deny(missing_docs)]
//! A crate for working with glob patterns and computing hashes or modification times
//! over matched files.
//!
//! This crate provides utilities for:
//! - Matching files using glob patterns with gitignore-style semantics
//! - Computing hashes over matched files (useful for cache invalidation)
//! - Finding the newest modification time among matched files
//! - Caching glob hash computations
//!
//! # Glob Semantics
//!
//! The glob matching in this crate uses gitignore-style patterns with some notable
//! behavioral tweaks to make it more intuitive for typical use cases:
//!
//! - **Relative patterns and rebasing**: Patterns containing `..` components (e.g.,
//!   `../src/*.rs`) are automatically **rebased** to work from a common ancestor directory
//!   called the **effective walk root**. For example, searching from `/project/subdir` with
//!   patterns `["../src/*.rs", "*.txt"]` will walk from `/project`, transforming the patterns
//!   to `src/*.rs` and `subdir/*.txt` respectively. This allows efficient single-pass matching
//!   even when patterns reference different directories.
//!
//! - **Global exclusions**: Negated patterns starting with `**/` (e.g., `!**/build.rs`)
//!   are treated as global exclusions and skip rebasing, ensuring they apply everywhere
//!   regardless of where the effective root ends up.
//!
//! - **Anchored literals**: Plain file names without meta characters (e.g., `config.toml`)
//!   are anchored to the search root, matching only at that location rather than
//!   anywhere in the tree. This differs from standard gitignore behavior.
//!
//! - **Hidden files**: By default, hidden files and directories (starting with `.`)
//!   are excluded unless explicitly included in the pattern.

mod glob_hash;
mod glob_hash_cache;
mod glob_mtime;
mod glob_set;

pub use glob_hash::{GlobHash, GlobHashError};
pub use glob_hash_cache::{GlobHashCache, GlobHashKey};
pub use glob_mtime::{GlobModificationTime, GlobModificationTimeError};
pub use glob_set::{GlobSet, GlobSetError};
