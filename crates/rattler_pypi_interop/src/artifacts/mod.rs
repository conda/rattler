//! Module containing data types for working with Python packaging artifacts.
//!
//! This module can be used to read and parse Python packaging artifacts like wheels
//! (binary distribution) and sdists (source distribution).
//!
//! Example of reading PKG-INFO from an sdist file:
//!
//! ```rust
//! use std::str::FromStr;
//! use std::path::Path;
//! use rattler_pypi_interop::artifacts::SDist;
//! use rattler_pypi_interop::types::NormalizedPackageName;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let path_to_wheel = Path::new(env!("CARGO_MANIFEST_DIR"))
//!         .join("test-data/sdists/rich-13.6.0.tar.gz");
//!     let normalized_package_name = NormalizedPackageName::from_str("rich")?;
//!
//!     let sdist = SDist::from_path(&path_to_wheel, &normalized_package_name)?;
//!     let (bytes, package_info) = sdist.read_package_info()?;
//!     println!("Length of bytes: {}", bytes.len());
//!     println!("{:?}", package_info.parsed.fields);
//!
//!     Ok(())
//! }
//! ```
//!
mod sdist;
mod stree;

/// Module for working with PyPA wheels. Contains the [`Wheel`] type, and related functionality.
pub mod wheel;

pub use sdist::SDist;
pub use stree::STree;
pub use wheel::Wheel;
