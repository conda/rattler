//! This module contains functions for working with pypi packaging repositories.
//!
//! Example of retrieving metadata about a package from a package index:
//!
//! ```rust
//! use std::str::FromStr;
//!
//! use rattler_pypi_interop::index::{
//!     ArtifactRequest, CheckAvailablePackages, PackageDb, PackageSourcesBuilder
//! };
//! use rattler_pypi_interop::types::NormalizedPackageName;
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     /// First, you need to setup everything related to the client that queries the index
//!     let pypi_url = Url::parse("https://pypi.org/simple/")?;
//!     let package_sources = PackageSourcesBuilder::new(pypi_url).build()?;
//!     let cache_dir = dirs::cache_dir().unwrap().join("rattler_pypi_interop");
//!     let client = ClientWithMiddleware::from(Client::new());
//!     let package_db = PackageDb::new(package_sources, client, &cache_dir, CheckAvailablePackages::Always)?;
//!
//!     /// After this we can set up and run the query
//!     let package_name = NormalizedPackageName::from_str("requests")?;
//!     let artifact_request = ArtifactRequest::FromIndex(package_name);
//!     let package_metadata = package_db.available_artifacts(artifact_request).await?;
//!
//!     /// Print what was returned
//!     println!("{:?}", package_metadata);
//!
//!     Ok(())
//! }
//! ```

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
