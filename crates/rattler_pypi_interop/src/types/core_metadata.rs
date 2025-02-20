// Implementation comes from https://github.com/njsmith/posy/blob/main/src/vocab/core_metadata.rs
// Licensed under MIT or Apache-2.0

use super::extra::ParseExtraError;
use crate::{
    types::Extra, types::PackageName, types::ParsePackageNameError, types::RFC822ish,
    types::Version, types::VersionSpecifiers,
};
use once_cell::sync::Lazy;
use pep440_rs::{VersionParseError, VersionSpecifiersParseError};
use pep508_rs::Requirement;
use std::{collections::HashSet, str::FromStr};
use thiserror::Error;

/// Holds the parsed PKG-INFO file.
pub struct PackageInfo {
    /// The parsed PKG-INFO file.
    pub parsed: RFC822ish,
}

impl PackageInfo {
    /// Parse the PKG-INFO file from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, WheelCoreMetaDataError> {
        let s = String::from_utf8_lossy(bytes);
        Ok(Self {
            parsed: RFC822ish::from_str(&s)?,
        })
    }

    /// Create a new `PackageInfo` from a parsed `RFC822ish`.
    pub fn new(parsed: RFC822ish) -> Self {
        Self { parsed }
    }
}

#[derive(Debug, Clone)]

/// The core metadata of a wheel.
pub struct WheelCoreMetadata {
    /// The name of the package
    pub name: PackageName,
    /// Version w.r.t to PEP440
    pub version: Version,
    /// Version of the metadata
    pub metadata_version: MetadataVersion,
    /// Requirements for this distribution
    /// Matches the Requires-Dist field
    pub requires_dist: Vec<Requirement>,
    /// Python requirement
    pub requires_python: Option<VersionSpecifiers>,
    /// Extras provided by this distribution
    pub extras: HashSet<Extra>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
/// Wrapper around a PEP440 version
/// specifically for the metadata version
pub struct MetadataVersion(pub Version);

impl MetadataVersion {
    /// We consider that this implements PEP643
    /// if the version is 2.3 or higher.
    pub fn implements_pep643(&self) -> bool {
        static VERSION_2_2: Lazy<MetadataVersion> = Lazy::new(|| {
            MetadataVersion(Version::from_str("2.2").expect("cannot parse 2.2 version string"))
        });

        if self < &VERSION_2_2 {
            return false;
        }
        true
    }
}

#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum WheelCoreMetaDataError {
    #[error(transparent)]
    FailedToParseMetadata(#[from] <RFC822ish as FromStr>::Err),

    #[error("missing key {0} in METADATA")]
    MissingKey(String),

    #[error("duplicate key {0} in METADATA")]
    DuplicateKey(String),

    #[error("invalid Metadata-Version: {0}")]
    InvalidMetadataVersion(VersionParseError),

    #[error("invalid Version: {0}")]
    InvalidVersion(VersionParseError),

    #[error("invalid Requires-Python: {0}")]
    InvalidRequiresPython(#[source] VersionSpecifiersParseError),

    #[error("unsupported METADATA version {0}")]
    UnsupportedVersion(Version),

    #[error(transparent)]
    InvalidPackageName(#[from] ParsePackageNameError),

    #[error("invalid extra identifier '{0}'")]
    InvalidExtra(String, #[source] ParseExtraError),

    #[error("{0}")]
    FailedToParse(String),
}

impl TryFrom<&[u8]> for WheelCoreMetadata {
    type Error = WheelCoreMetaDataError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        PackageInfo::from_bytes(value)?.try_into()
    }
}

impl TryFrom<PackageInfo> for WheelCoreMetadata {
    type Error = WheelCoreMetaDataError;

    fn try_from(value: PackageInfo) -> Result<Self, Self::Error> {
        let (name, version, metadata_version, mut parsed) = parse_common(value)?;

        let mut requires_dist = Vec::new();
        for req_str in parsed.take_all("Requires-Dist") {
            match req_str.parse() {
                Err(e) => {
                    tracing::warn!("ignoring Requires-Dist: {req_str}, failed to parse: {e}");
                }
                Ok(req) => requires_dist.push(req),
            }
        }

        let requires_python = parsed
            .maybe_take("Requires-Python")
            .map_err(|_err| WheelCoreMetaDataError::DuplicateKey(String::from("Requires-Python")))?
            .as_deref()
            .map(VersionSpecifiers::from_str)
            .transpose()
            .map_err(WheelCoreMetaDataError::InvalidRequiresPython)?;

        let mut extras: HashSet<Extra> = HashSet::new();
        for extra in parsed.take_all("Provides-Extra").drain(..) {
            extras.insert(
                extra
                    .parse()
                    .map_err(|e| WheelCoreMetaDataError::InvalidExtra(extra, e))?,
            );
        }

        Ok(WheelCoreMetadata {
            name,
            version,
            metadata_version,
            requires_dist,
            requires_python,
            extras,
        })
    }
}

fn parse_common(
    input: PackageInfo,
) -> Result<(PackageName, Version, MetadataVersion, RFC822ish), WheelCoreMetaDataError> {
    let mut parsed = input.parsed;

    let next_major_metadata_version: Lazy<Version> = Lazy::new(|| Version::from_str("3").unwrap());

    // Quoth https://packaging.python.org/specifications/core-metadata:
    // "Automated tools consuming metadata SHOULD warn if metadata_version
    // is greater than the highest version they support, and MUST fail if
    // metadata_version has a greater major version than the highest
    // version they support (as described in PEP 440, the major version is
    // the value before the first dot)."
    //
    // We do the MUST, but I think I disagree about warning on
    // unrecognized minor revisions. If it's a minor revision, then by
    // definition old software is supposed to be able to handle it "well
    // enough". The only purpose of the warning would be to alert users
    // that they might want to upgrade, or to alert the tool authors that
    // there's a new metadata release. But for users, there are better
    // ways to nudge them to upgrade (e.g. checking on startup, like
    // pip does), and new metadata releases are so rare and so
    // much-discussed beforehand that if a tool's authors don't know
    // about it it's because the tool is abandoned anyway.
    let metadata_version = parsed
        .take("Metadata-Version")
        .map_err(|_err| WheelCoreMetaDataError::MissingKey(String::from("Metadata-Version")))?;
    let metadata_version: Version = metadata_version
        .parse()
        .map_err(WheelCoreMetaDataError::InvalidMetadataVersion)?;
    if metadata_version >= *next_major_metadata_version {
        return Err(WheelCoreMetaDataError::UnsupportedVersion(metadata_version));
    }

    let version_str = parsed
        .take("Version")
        .map_err(|_err| WheelCoreMetaDataError::MissingKey(String::from("Version")))?;

    Ok((
        parsed
            .take("Name")
            .map_err(|_err| WheelCoreMetaDataError::MissingKey(String::from("Name")))?
            .parse()?,
        version_str
            .parse()
            .map_err(WheelCoreMetaDataError::InvalidVersion)?,
        MetadataVersion(metadata_version),
        parsed,
    ))
}
