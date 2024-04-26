mod deserialize;
mod serialize;
mod v3;

use super::{LockFile, UrlOrPath};
use rattler_conda_types::Platform;
use serde::de::Error;
use serde_yaml::Value;
use std::str::FromStr;
use v3::parse_v3_or_lower;

// Version 2: dependencies are now arrays instead of maps
// Version 3: pip has been renamed to pypi
// Version 4: Complete overhaul of the lock-file with support for multienv.
const FILE_VERSION: u64 = 4;

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum ParseCondaLockError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    ParseError(#[from] serde_yaml::Error),

    #[error("found newer lockfile format version {lock_file_version}, but only up to including version {max_supported_version} is supported.")]
    IncompatibleVersion {
        lock_file_version: u64,
        max_supported_version: u64,
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
        let version = document
            .get("version")
            .ok_or_else(|| {
                ParseCondaLockError::ParseError(serde_yaml::Error::custom(
                    "missing `version` field in lock file",
                ))
            })
            .and_then(|v| {
                v.as_u64().ok_or_else(|| {
                    ParseCondaLockError::ParseError(serde_yaml::Error::custom(
                        "`version` field in lock file is not an integer",
                    ))
                })
            })?;

        if version > FILE_VERSION {
            return Err(ParseCondaLockError::IncompatibleVersion {
                lock_file_version: version,
                max_supported_version: FILE_VERSION,
            });
        }

        if version <= 3 {
            parse_v3_or_lower(document)
        } else {
            deserialize::parse_from_document(document)
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

        insta::assert_snapshot!(format!("{}", err), @"found newer lockfile format version 1000, but only up to including version 4 is supported.");
    }
}
