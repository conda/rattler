mod deserialize;
mod models;
mod serialize;
mod v3;

use std::path::Path;

use serde::de::Error;
use serde_yaml::Value;
use v3::parse_v3_or_lower;

use super::{LockFile, UrlOrPath};
use crate::{file_format_version::FileFormatVersion, parse::deserialize::parse_from_document_v5};

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum ParseCondaLockError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    ParseError(#[from] serde_yaml::Error),

    #[error("found newer lockfile format version {lock_file_version}, but only up to including version {max_supported_version} is supported")]
    IncompatibleVersion {
        lock_file_version: u64,
        max_supported_version: FileFormatVersion,
    },

    #[error("environment {environment} and platform {platform} refers to a package that does not exist: {location}")]
    MissingPackage {
        environment: String,
        platform: String,
        location: String,
    },

    #[error("Python requirement parsing failed")]
    Pep508Error(#[from] pep508_rs::Pep508Error),

    #[error(transparent)]
    InvalidPypiPackageName(#[from] pep508_rs::InvalidNameError),

    #[error(transparent)]
    InvalidPlatform(#[from] crate::platform::ParsePlatformError),

    #[error("Duplicate platform name `{0}` found")]
    DuplicatePlatformName(String),

    #[error("missing field `{0}` for package {1}")]
    MissingField(String, UrlOrPath),

    #[error("`platforms` were not supported in lockfile version {0}")]
    UnexpectedPlatforms(FileFormatVersion),

    #[error("Environment `{environment}` is using an unknown platform `{platform}`")]
    UnknownPlatform {
        environment: String,
        platform: String,
    },

    /// The location of the conda package cannot be converted to a URL
    #[error(transparent)]
    LocationToUrlConversionError(#[from] file_url::FileURLParseError),
}

pub(crate) fn from_str_with_base_directory(
    s: &str,
    base_dir: Option<&Path>,
) -> Result<LockFile, ParseCondaLockError> {
    // First parse the document to a `serde_yaml::Value`.
    let document: Value = serde_yaml::from_str(s).map_err(ParseCondaLockError::ParseError)?;

    // Read the version number from the document
    let version: FileFormatVersion = document
        .get("version")
        .ok_or_else(|| {
            ParseCondaLockError::ParseError(serde_yaml::Error::custom(
                "missing `version` field in lock file",
            ))
        })
        .and_then(|v| {
            let v = v.as_u64().ok_or_else(|| {
                ParseCondaLockError::ParseError(serde_yaml::Error::custom(
                    "`version` field in lock file is not an integer",
                ))
            })?;

            FileFormatVersion::try_from(v)
        })?;

    match version {
        FileFormatVersion::V1 | FileFormatVersion::V2 | FileFormatVersion::V3 => {
            parse_v3_or_lower(document, version)
        }
        FileFormatVersion::V4 | FileFormatVersion::V5 => parse_from_document_v5(document, version),
        FileFormatVersion::V6 => deserialize::parse_from_document_v6(document, base_dir),
        FileFormatVersion::V7 => deserialize::parse_from_document_v7(document, base_dir),
    }
}

/// A helper structs to differentiate between the serde code paths for different
/// versions.
struct V5;
struct V6;
struct V7;

#[cfg(test)]
mod test {
    use std::path::Path;

    use super::*;

    #[test]
    fn test_forward_compatibility() {
        let err = LockFile::from_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../test-data/conda-lock/forward-compatible-lock.yml"),
        )
        .err()
        .unwrap();

        insta::assert_snapshot!(format!("{}", err), @"found newer lockfile format version 1000, but only up to including version 7 is supported");
    }

    // This test verifies the deterministic ordering of lock files. It does so by
    // comparing the serialized YAML output of two lock files: one with the
    // original ordering and another with a shuffled ordering. The test ensures
    // that, despite the initial difference in order, the serialization process
    // results in identical YAML strings.
    #[test]
    fn test_deterministic_lock_file_ordering() {
        let lock_file_original = LockFile::from_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../test-data/conda-lock/v5/stability-original.yml"),
        )
        .unwrap();
        let lock_file_shuffled = LockFile::from_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../test-data/conda-lock/v5/stability-shuffled.yml"),
        )
        .unwrap();

        let output_original =
            serde_yaml::to_string(&lock_file_original).expect("could not deserialize");
        let output_shuffled =
            serde_yaml::to_string(&lock_file_shuffled).expect("could not deserialize");

        assert_eq!(output_original, output_shuffled);
    }
}
