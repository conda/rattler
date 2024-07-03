mod deserialize;
mod serialize;
mod v3;

use super::{LockFile, UrlOrPath};
use crate::file_format_version::FileFormatVersion;
use rattler_conda_types::Platform;
use serde::de::Error;
use serde_yaml::Value;
use std::str::FromStr;
use v3::parse_v3_or_lower;

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

    #[error("environment {0} and platform {1} refers to a package that does not exist: {2}")]
    MissingPackage(String, Platform, UrlOrPath),

    #[error(transparent)]
    InvalidPypiPackageName(#[from] pep508_rs::InvalidNameError),
}

impl FromStr for LockFile {
    type Err = ParseCondaLockError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
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

        if version <= FileFormatVersion::V3 {
            parse_v3_or_lower(document, version)
        } else {
            deserialize::parse_from_document(document, version)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_forward_compatibility() {
        let err = LockFile::from_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../test-data/conda-lock/forward-compatible-lock.yml"),
        )
        .err()
        .unwrap();

        insta::assert_snapshot!(format!("{}", err), @"found newer lockfile format version 1000, but only up to including version 5 is supported");
    }

    // This test verifies the deterministic ordering of lock files. It does so by comparing the serialized
    // YAML output of two lock files: one with the original ordering and another with a shuffled ordering.
    // The test ensures that, despite the initial difference in order, the serialization process results
    // in identical YAML strings.
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
