use super::{CondaLock, LockMeta, LockedDependency, LockedDependencyKind};
use serde::de::Error;
use serde::{Deserialize, Serialize, Serializer};
use serde_yaml::Value;
use std::cmp::Ordering;
use std::str::FromStr;

// Version 2: dependencies are now arrays instead of maps
// Version 3: pip has been renamed to pypi
const FILE_VERSION: u64 = 3;

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
}

impl FromStr for CondaLock {
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

        if version > FILE_VERSION as u64 {
            return Err(ParseCondaLockError::IncompatibleVersion {
                lock_file_version: version,
                max_supported_version: FILE_VERSION as u64,
            });
        }

        // Then parse the document to a `CondaLock`
        #[derive(Deserialize)]
        struct Raw {
            metadata: LockMeta,
            package: Vec<LockedDependency>,
        }

        let raw: Raw = serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
        Ok(Self {
            metadata: raw.metadata,
            package: raw.package,
        })
    }
}

impl Serialize for CondaLock {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Raw<'a> {
            version: u64,
            metadata: &'a LockMeta,
            package: Vec<&'a LockedDependency>,
        }

        // Sort all packages in alphabetical order. We choose to use alphabetic order instead of
        // topological because the alphabetic order will create smaller diffs when packages change
        // or are added.
        // See: https://github.com/conda/conda-lock/issues/491
        let mut sorted_deps = self.package.iter().collect::<Vec<_>>();
        sorted_deps.sort_by(|&a, &b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.platform.cmp(&b.platform))
                .then_with(|| a.version.cmp(&b.version))
                .then_with(|| match (&a.kind, &b.kind) {
                    (LockedDependencyKind::Conda(a), LockedDependencyKind::Conda(b)) => {
                        a.build.cmp(&b.build)
                    }
                    (LockedDependencyKind::Pypi(_), LockedDependencyKind::Pypi(_)) => {
                        Ordering::Equal
                    }
                    (LockedDependencyKind::Pypi(_), _) => Ordering::Less,
                    (_, LockedDependencyKind::Pypi(_)) => Ordering::Greater,
                })
        });

        let raw = Raw {
            version: FILE_VERSION,
            metadata: &self.metadata,
            package: sorted_deps,
        };

        raw.serialize(serializer)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::Path;

    #[test]
    fn read_conda_lock() {
        let err = CondaLock::from_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../test-data/conda-lock/forward-compatible-lock.yml"),
        )
        .unwrap_err();

        insta::assert_snapshot!(format!("{}", err), @"found newer lockfile format version 1000, but only up to including version 3 is supported.");
    }
}
