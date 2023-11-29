use super::{CondaLock, LockMeta, LockedDependency, LockedDependencyKind};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::path::PathBuf;

// Version 2: dependencies are now arrays instead of maps
// Version 3: pip has been renamed to pypi
const FILE_VERSION: u32 = 3;

/// A helper struct to deserialize the version field of the lock file and provide potential errors
/// in-line.
#[derive(Serialize)]
#[serde(transparent)]
struct Version(u32);

impl Default for Version {
    fn default() -> Self {
        Self(FILE_VERSION)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = u32::deserialize(deserializer)?;

        if version > FILE_VERSION {
            let binary = if cfg!(test) {
                // When running tests, use a hardcoded name or an environment variable
                String::from(env!("CARGO_PKG_NAME"))
            } else {
                // When not testing, use the current executable's name, e.g. "pixi" for pixi.
                let binary_path =
                    std::env::current_exe().unwrap_or(PathBuf::from(env!("CARGO_PKG_NAME")));
                let binary_file_name = binary_path
                    .file_name()
                    .unwrap_or(env!("CARGO_PKG_NAME").as_ref());
                binary_file_name.to_string_lossy().into_owned()
            };
            return Err(D::Error::custom(format!(
                "found newer lockfile format version {}, but only up to including version {} is supported.\n\
                Please update your {} version.",
                version, FILE_VERSION, binary
            )));
        }

        Ok(Self(version))
    }
}

impl<'de> Deserialize<'de> for CondaLock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[allow(dead_code)]
        #[derive(Deserialize)]
        struct Raw {
            version: Version,
            metadata: LockMeta,
            package: Vec<LockedDependency>,
        }

        let raw = Raw::deserialize(deserializer)?;
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
            version: Version,
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
            version: Default::default(),
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

        insta::assert_snapshot!(format!("{}", err), @"found newer lockfile format version 1000, but only up to including version 3 is supported.\nPlease update your rattler_lock version.");
    }
}
