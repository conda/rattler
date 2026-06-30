#![deny(missing_docs, dead_code)]

//! This crate provides a set of functions and data types for working with Python packages installed via
//! `PyPI` compatible installers (e.g. `pip`, `uv`, `poetry`, etc.).
//!
//! Some of the things you can do with this crate include:
//!
//! - Querying a `PyPI` repository for package metadata.
//! - Reading Python package metadata from locally installed packages.
//!
//! <div class="warning">
//!
//!   This crate ports over a subset of functionality originally available in the
//!   [`rattler_installs_packages`] crate that is also part of the [`rip`] CLI tool.
//!
//! </div>
//!
//! [`rip`]: https://github.com/prefix-dev/rip/
//! [`rattler_installs_packages`]: https://docs.rs/rattler_installs_packages/latest/rattler_installs_packages/

// Private modules
mod utils;

// Public modules
pub mod artifacts;
pub mod index;
pub mod python_env;
pub mod types;
