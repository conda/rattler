#![deny(missing_docs)]
//! Parse, compare, and match versions used by conda packages.
//!
//! Version literals and their ordering follow
//! [CEP 33](https://conda.org/learn/ceps/cep-0033). Conda versions have
//! comparison rules that differ from semantic versioning:
//! they support epochs, local versions, arbitrary alphanumeric components, and
//! special `dev` and `post` identifiers. [`Version`] implements those rules.
//! [`VersionSpec`] parses the version-matching portion of conda `MatchSpecs`,
//! specified by [CEP 29](https://conda.org/learn/ceps/cep-0029).
//!
//! ```
//! use rattler_conda_version::{ParseStrictness, Version, VersionSpec};
//! use std::str::FromStr;
//!
//! let version = Version::from_str("1.2.3rc1").unwrap();
//! let requirement = VersionSpec::from_str(">=1.2,<2", ParseStrictness::Lenient).unwrap();
//!
//! assert!(requirement.matches(&version));
//! ```
//!
//! # Optional `SemVer` interoperability
//!
//! Enable the `semver` feature to convert between [`Version`] and
//! `semver::Version`. Converting a conda version to `SemVer` is fallible because
//! conda version literals can express forms that `SemVer` cannot represent.

mod parse_strictness;
pub mod version;
pub mod version_spec;

pub use parse_strictness::ParseStrictness;
pub use version::Version;
pub use version_spec::VersionSpec;
