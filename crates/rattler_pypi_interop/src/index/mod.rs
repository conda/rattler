//! This module contains functions for working with PyPA packaging repositories.
//!
//! TODO: Examples to include:
//!          - PackageDb

mod file_store;

mod http;
mod lazy_metadata;
mod package_database;
mod package_sources;

pub mod html;
pub use package_database::{ArtifactRequest, CheckAvailablePackages, PackageDb};
pub use package_sources::{PackageSources, PackageSourcesBuilder};

pub use self::http::CacheMode;
pub use html::parse_hash;
