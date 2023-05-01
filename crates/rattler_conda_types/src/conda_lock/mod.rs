//! This is the definitions for a conda-lock file format
//! It is modeled on the definitions found at: [conda-lock models](https://github.com/conda/conda-lock/blob/main/conda_lock/lockfile/models.py)
//! Most names were kept the same as in the models file. So you can refer to those exactly.
//! However, some types were added to enforce a bit more type safety.
use crate::conda_lock::PackageHashes::{Md5, Md5Sha256, Sha256};
use crate::{ParsePlatformError, Platform};
use rattler_digest::serde::SerializableHash;
use rattler_digest::{Md5Hash, Sha256Hash};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;
use url::Url;

pub mod builder;
mod content_hash;

/// Default version for the conda-lock file format
const fn default_version() -> u32 {
    1
}

/// Represents the conda-lock file
/// Contains the metadata regarding the lock files
/// also the locked packages
#[derive(Serialize, Deserialize)]
pub struct CondaLock {
    /// Metadata for the lock file
    pub metadata: LockMeta,

    /// Locked packages
    pub package: Vec<LockedDependency>,

    /// Version of the conda-lock file format
    #[serde(default = "default_version")]
    pub version: u32,
}

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum ParseCondaLockError {
    /// The platform could not be parsed
    #[error(transparent)]
    InvalidPlatform(#[from] ParsePlatformError),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    ParseError(#[from] serde_yaml::Error),
}

impl FromStr for CondaLock {
    type Err = ParseCondaLockError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_yaml::from_str(s).map_err(ParseCondaLockError::ParseError)
    }
}

impl CondaLock {
    /// This returns the packages for the specific platform
    /// Will return an empty iterator if no packages exist in
    /// this lock file for this specific platform
    pub fn packages_for_platform(
        &self,
        platform: Platform,
    ) -> impl Iterator<Item = &LockedDependency> {
        self.package.iter().filter(move |p| p.platform == platform)
    }

    /// Parses an conda-lock file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<Self, ParseCondaLockError> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses an conda-lock file from a file.
    pub fn from_path(path: &Path) -> Result<Self, ParseCondaLockError> {
        Self::from_reader(File::open(path)?)
    }

    /// Writes the conda lock to a file
    pub fn to_path(&self, path: &Path) -> Result<(), std::io::Error> {
        let file = std::fs::File::create(path)?;
        serde_yaml::to_writer(file, self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
    }
}

#[derive(Serialize, Deserialize)]
/// Metadata for the [`CondaLock`] file
pub struct LockMeta {
    /// Hash of dependencies for each target platform
    pub content_hash: HashMap<Platform, String>,
    /// Channels used to resolve dependencies
    pub channels: Vec<Channel>,
    /// The platforms this lock file supports
    pub platforms: Vec<Platform>,
    /// Paths to source files, relative to the parent directory of the lockfile
    pub sources: Vec<String>,
    /// Metadata dealing with the time lockfile was created
    pub time_metadata: Option<TimeMeta>,
    /// Metadata dealing with the git repo the lockfile was created in and the user that created it
    pub git_metadata: Option<GitMeta>,
    /// Metadata dealing with the input files used to create the lockfile
    pub inputs_metadata: Option<HashMap<String, PackageHashes>>,
    /// Custom metadata provided by the user to be added to the lockfile
    pub custom_metadata: Option<HashMap<String, String>>,
}

/// Stores information about when the lockfile was generated
#[derive(Serialize, Deserialize)]
pub struct TimeMeta {
    /// Time stamp of lock-file creation format
    // TODO: I think this is UTC time, change this later, conda-lock is not really using this now
    pub created_at: String,
}

/// Stores information about the git repo the lockfile is being generated in (if applicable) and
/// the git user generating the file.
#[derive(Serialize, Deserialize)]
pub struct GitMeta {
    /// Git user.name field of global config
    pub git_user_name: String,
    /// Git user.email field of global config
    pub git_user_email: String,
    /// sha256 hash of the most recent git commit that modified one of the input files for this lockfile
    pub git_sha: String,
}

/// Represents whether this is a dependency managed by pip or conda
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Manager {
    /// The "conda" manager
    Conda,
    /// The pip manager
    Pip,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Hash)]
/// This is basically a MatchSpec but will never contain the package name
pub struct VersionConstraint(String);

impl Display for VersionConstraint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// This implementation of the `Deserialize` trait for the `PackageHashes` struct
///
/// It expects the input to have either a `md5` field, a `sha256` field, or both.
/// If both fields are present, it constructs a `Md5Sha256` instance with their values.
/// If only the `md5` field is present, it constructs a `Md5` instance with its value.
/// If only the `sha256` field is present, it constructs a `Sha256` instance with its value.
/// If neither field is present it returns an error
pub enum PackageHashes {
    /// Contains an MD5 hash
    Md5(Md5Hash),
    /// Contains as Sha256 Hash
    Sha256(Sha256Hash),
    /// Contains both hashes
    Md5Sha256(Md5Hash, Sha256Hash),
}

#[derive(Serialize, Deserialize)]
struct RawPackageHashes {
    md5: Option<SerializableHash<rattler_digest::Md5>>,
    sha256: Option<SerializableHash<rattler_digest::Sha256>>,
}

impl Serialize for PackageHashes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let raw = match self {
            Md5(hash) => RawPackageHashes {
                md5: Some(SerializableHash::from(*hash)),
                sha256: None,
            },
            Sha256(hash) => RawPackageHashes {
                md5: None,
                sha256: Some(SerializableHash::from(*hash)),
            },
            Md5Sha256(md5hash, sha) => RawPackageHashes {
                md5: Some(SerializableHash::from(*md5hash)),
                sha256: Some(SerializableHash::from(*sha)),
            },
        };
        raw.serialize(serializer)
    }
}

// This implementation of the `Deserialize` trait for the `PackageHashes` struct
//
// It expects the input to have either a `md5` field, a `sha256` field, or both.
// If both fields are present, it constructs a `Md5Sha256` instance with their values.
// If only the `md5` field is present, it constructs a `Md5` instance with its value.
// If only the `sha256` field is present, it constructs a `Sha256` instance with its value.
// If neither field is present it returns an error
impl<'de> Deserialize<'de> for PackageHashes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let temp = RawPackageHashes::deserialize(deserializer)?;
        Ok(match (temp.md5, temp.sha256) {
            (Some(md5), Some(sha)) => Md5Sha256(md5.into(), sha.into()),
            (Some(md5), None) => Md5(md5.into()),
            (None, Some(sha)) => Sha256(sha.into()),
            _ => return Err(Error::custom("Expected `sha256` field `md5` field or both")),
        })
    }
}

/// Default category of a locked package
fn default_category() -> String {
    "main".to_string()
}

/// A locked single dependency / package
#[derive(Serialize, Deserialize)]
pub struct LockedDependency {
    /// Package name of dependency
    pub name: String,
    /// Locked version
    pub version: String,
    /// Pip or Conda managed
    pub manager: Manager,
    /// What platform is this package for
    pub platform: Platform,
    /// What are its own dependencies mapping name to version constraint
    pub dependencies: HashMap<String, VersionConstraint>,
    /// URL to find it at
    pub url: Url,
    /// Hashes of the package
    pub hash: PackageHashes,
    /// Is the dependency optional
    pub optional: bool,
    /// Used for pip packages
    #[serde(default = "default_category")]
    pub category: String,
    /// ???
    pub source: Option<Url>,
    /// Build string
    pub build: Option<String>,
}

/// The URL for the dependency (currently only used for pip packages)
#[derive(Serialize, Deserialize)]
pub struct DependencySource {
    // According to:
    // https://github.com/conda/conda-lock/blob/854fca9923faae95dc2ddd1633d26fd6b8c2a82d/conda_lock/lockfile/models.py#L27
    // It also has a type field but this can only be url at the moment
    // so leaving it out for now!
    /// URL of the dependency
    pub url: Url,
}

/// The conda channel that was used for the dependency
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Channel {
    /// Called `url` but can also be the name of the channel e.g. `conda-forge`
    pub url: String,
    /// Used env vars for the channel (e.g. hints for passwords or other secrets)
    pub used_env_vars: Vec<String>,
}

impl From<String> for Channel {
    fn from(url: String) -> Self {
        Self {
            url,
            used_env_vars: Default::default(),
        }
    }
}

impl From<&str> for Channel {
    fn from(url: &str) -> Self {
        Self {
            url: url.to_string(),
            used_env_vars: Default::default(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::PackageHashes;
    use crate::conda_lock::CondaLock;
    use crate::Platform;
    use insta::assert_yaml_snapshot;
    use serde_yaml::from_str;
    use std::path::Path;

    #[test]
    fn test_package_hashes() {
        let yaml = r#"
          md5: 4eccaeba205f0aed9ac3a9ea58568ca3
          sha256: f240217476e148e825420c6bc3a0c0efb08c0718b7042fae960400c02af858a3
    "#;

        let result: PackageHashes = from_str(yaml).unwrap();
        assert!(matches!(result, PackageHashes::Md5Sha256(_, _)));

        let yaml = r#"
          md5: 4eccaeba205f0aed9ac3a9ea58568ca3
    "#;

        let result: PackageHashes = from_str(yaml).unwrap();
        assert!(matches!(result, PackageHashes::Md5(_)));

        let yaml = r#"
          sha256: f240217476e148e825420c6bc3a0c0efb08c0718b7042fae960400c02af858a3
    "#;

        let result: PackageHashes = from_str(yaml).unwrap();
        assert!(matches!(result, PackageHashes::Sha256(_)));
    }

    fn lock_file_path() -> String {
        format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "../../test-data/conda-lock/numpy-conda-lock.yml"
        )
    }

    fn lock_file_path_python() -> String {
        format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "../../test-data/conda-lock/python-conda-lock.yml"
        )
    }

    #[test]
    fn read_conda_lock() {
        // Try to read conda_lock
        let conda_lock = CondaLock::from_path(Path::new(&lock_file_path())).unwrap();
        // Make sure that we have parsed some packages
        insta::with_settings!({sort_maps => true}, {
        insta::assert_yaml_snapshot!(conda_lock);
        });
    }

    #[test]
    fn read_conda_lock_python() {
        // Try to read conda_lock
        let conda_lock = CondaLock::from_path(Path::new(&lock_file_path_python())).unwrap();
        // Make sure that we have parsed some packages
        insta::with_settings!({sort_maps => true}, {
        insta::assert_yaml_snapshot!(conda_lock);
        });
    }

    #[test]
    fn packages_for_platform() {
        // Try to read conda_lock
        let conda_lock = CondaLock::from_path(Path::new(&lock_file_path())).unwrap();
        // Make sure that we have parsed some packages
        assert!(!conda_lock.package.is_empty());
        insta::with_settings!({sort_maps => true}, {
        assert_yaml_snapshot!(
            conda_lock
                .packages_for_platform(Platform::Linux64)
                .collect::<Vec<_>>()
        );
        assert_yaml_snapshot!(
            conda_lock
                .packages_for_platform(Platform::Osx64)
                .collect::<Vec<_>>()
        );
        assert_yaml_snapshot!(
            conda_lock
                .packages_for_platform(Platform::OsxArm64)
                .collect::<Vec<_>>()
        );
            })
    }
}
