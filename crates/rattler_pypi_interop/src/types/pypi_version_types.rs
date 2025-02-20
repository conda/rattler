//! This module contains types that are used to represent versions and version sets
//! these are used by the [`resolvo`] crate to resolve dependencies.

use crate::types::{Extra, NormalizedPackageName};
use pep440_rs::Version;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use url::Url;

/// This is a wrapper around [`Version`]. It can also be a direct url to a package.
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum PypiVersion {
    /// Version of artifact
    Version {
        /// Version of Artifact
        version: Version,

        /// This field is true if there are only pre-releases available for this
        /// package or if a spec explicitly enabled pre-releases for this package.
        /// For example, if the package
        /// `foo` has only versions `foo-1.0.0a1` and `foo-1.0.0a2` then this
        /// will be true. This allows us later to match against this version and
        /// allow the selection of pre-releases. Additionally, this is also true
        /// if any of the explicitly mentioned specs (by the user) contains a
        /// prerelease (for example c>0.0.0b0) contains the `b0` which signifies
        /// a pre-release.
        package_allows_prerelease: bool,
    },
    /// Direct reference for artifact
    Url(Url),
}

impl PypiVersion {
    /// Return if there are any prereleases for version
    pub fn any_prerelease(&self) -> bool {
        match self {
            PypiVersion::Url(_) => false,
            PypiVersion::Version { version, .. } => version.any_prerelease(),
        }
    }

    /// Return if pypi version is git url version
    pub fn is_git(&self) -> bool {
        match self {
            PypiVersion::Version { .. } => false,
            PypiVersion::Url(url) => url.scheme().contains("git"),
        }
    }
}

impl Display for PypiVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PypiVersion::Version { version, .. } => write!(f, "{version}"),
            PypiVersion::Url(u) => write!(f, "{u}"),
        }
    }
}

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
/// This can either be a base package name or with an extra
/// this is used to support optional dependencies
pub enum PypiPackageName {
    /// Regular dependency
    Base(NormalizedPackageName),
    /// Optional dependency
    Extra(NormalizedPackageName, Extra),
}

impl PypiPackageName {
    /// Returns the actual package (normalized) name without the extra
    pub fn base(&self) -> &NormalizedPackageName {
        match self {
            PypiPackageName::Base(normalized) | PypiPackageName::Extra(normalized, _) => normalized,
        }
    }

    /// Retrieves the extra if it is available
    pub fn extra(&self) -> Option<&Extra> {
        match self {
            PypiPackageName::Base(_) => None,
            PypiPackageName::Extra(_, e) => Some(e),
        }
    }
}

impl Display for PypiPackageName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PypiPackageName::Base(name) => write!(f, "{name}"),
            PypiPackageName::Extra(name, extra) => write!(f, "{name}[{}]", extra.as_str()),
        }
    }
}
