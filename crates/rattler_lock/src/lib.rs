//! This is the definitions for a conda-lock file format
//! It is modeled on the definitions found at: [conda-lock models](https://github.com/conda/conda-lock/blob/main/conda_lock/lockfile/models.py)
//! Most names were kept the same as in the models file. So you can refer to those exactly.
//! However, some types were added to enforce a bit more type safety.
use ::serde::{Deserialize, Serialize};
use indexmap::IndexMap;
use rattler_conda_types::{MatchSpec, PackageName};
use rattler_conda_types::{NoArchType, Platform, RepoDataRecord};
use serde_with::serde_as;
use std::{collections::BTreeMap, io::Read, path::Path, str::FromStr};
use url::Url;

pub mod builder;
mod conda;
mod content_hash;
mod hash;
mod pip;
mod serde;
mod utils;

use crate::conda::ConversionError;
pub use conda::CondaLockedDependency;
pub use hash::PackageHashes;
pub use pip::PipLockedDependency;

/// Represents the conda-lock file
/// Contains the metadata regarding the lock files
/// also the locked packages
#[derive(Clone, Debug)]
pub struct CondaLock {
    /// Metadata for the lock file
    pub metadata: LockMeta,

    /// Locked packages
    pub package: Vec<LockedDependency>,
}

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum ParseCondaLockError {
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
        let source = std::fs::read_to_string(path)?;
        Self::from_str(&source)
    }

    /// Writes the conda lock to a file
    pub fn to_path(&self, path: &Path) -> Result<(), std::io::Error> {
        let file = std::fs::File::create(path)?;
        serde_yaml::to_writer(file, self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
/// Metadata for the [`CondaLock`] file
pub struct LockMeta {
    /// Hash of dependencies for each target platform
    pub content_hash: BTreeMap<Platform, String>,
    /// Channels used to resolve dependencies
    pub channels: Vec<Channel>,
    /// The platforms this lock file supports
    #[serde_as(as = "crate::utils::serde::Ordered<_>")]
    pub platforms: Vec<Platform>,
    /// Paths to source files, relative to the parent directory of the lockfile
    pub sources: Vec<String>,
    /// Metadata dealing with the time lockfile was created
    pub time_metadata: Option<TimeMeta>,
    /// Metadata dealing with the git repo the lockfile was created in and the user that created it
    pub git_metadata: Option<GitMeta>,
    /// Metadata dealing with the input files used to create the lockfile
    pub inputs_metadata: Option<IndexMap<String, PackageHashes>>,
    /// Custom metadata provided by the user to be added to the lockfile
    pub custom_metadata: Option<IndexMap<String, String>>,
}

/// Stores information about when the lockfile was generated
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct TimeMeta {
    /// Time stamp of lock-file creation format
    // TODO: I think this is UTC time, change this later, conda-lock is not really using this now
    pub created_at: String,
}

/// Stores information about the git repo the lockfile is being generated in (if applicable) and
/// the git user generating the file.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct GitMeta {
    /// Git user.name field of global config
    pub git_user_name: String,
    /// Git user.email field of global config
    pub git_user_email: String,
    /// sha256 hash of the most recent git commit that modified one of the input files for this lockfile
    pub git_sha: String,
}

/// Default category of a locked package
fn default_category() -> String {
    "main".to_string()
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct LockedDependency {
    /// What platform is this package for (different to other places in the conda ecosystem,
    /// this actually represents the _full_ subdir (incl. arch))
    pub platform: Platform,

    /// Normalized package name of dependency
    pub name: String,

    /// Locked version
    pub version: String,

    /// Defines the category under which this dependency is included
    #[serde(default = "default_category")]
    pub category: String,

    /// Defines ecosystem specific information.
    #[serde(flatten)]
    pub kind: LockedDependencyKind,
}

impl LockedDependency {
    /// Returns a reference to the internal [`CondaLockedDependency`] if this instance represents
    /// a conda package.
    pub fn as_conda(&self) -> Option<&CondaLockedDependency> {
        match &self.kind {
            LockedDependencyKind::Conda(conda) => Some(conda),
            LockedDependencyKind::Pip(_) => None,
        }
    }

    /// Returns a reference to the internal [`PipLockedDependency`] if this instance represents
    /// a pip package.
    pub fn as_pip(&self) -> Option<&PipLockedDependency> {
        match &self.kind {
            LockedDependencyKind::Conda(_) => None,
            LockedDependencyKind::Pip(pip) => Some(pip),
        }
    }

    /// Returns true if this instance represents a conda package.
    pub fn is_conda(&self) -> bool {
        matches!(self.kind, LockedDependencyKind::Conda(_))
    }

    /// Returns true if this instance represents a pip package.
    pub fn is_pip(&self) -> bool {
        matches!(self.kind, LockedDependencyKind::Pip(_))
    }
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
#[serde(tag = "manager", rename_all = "snake_case")]
pub enum LockedDependencyKind {
    Conda(CondaLockedDependency),
    Pip(PipLockedDependency),
}

impl From<CondaLockedDependency> for LockedDependencyKind {
    fn from(value: CondaLockedDependency) -> Self {
        LockedDependencyKind::Conda(value)
    }
}

impl From<PipLockedDependency> for LockedDependencyKind {
    fn from(value: PipLockedDependency) -> Self {
        LockedDependencyKind::Pip(value)
    }
}

/// The URL for the dependency (currently only used for pip packages)
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Hash)]
pub struct DependencySource {
    // According to:
    // https://github.com/conda/conda-lock/blob/854fca9923faae95dc2ddd1633d26fd6b8c2a82d/conda_lock/lockfile/models.py#L27
    // It also has a type field but this can only be url at the moment
    // so leaving it out for now!
    /// URL of the dependency
    pub url: Url,
}

/// The conda channel that was used for the dependency
#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Channel {
    /// Called `url` but can also be the name of the channel e.g. `conda-forge`
    pub url: String,
    /// Used env vars for the channel (e.g. hints for passwords or other secrets)
    #[serde_as(as = "crate::utils::serde::Ordered<_>")]
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

impl CondaLock {
    /// Returns all the packages in the lock-file for a certain platform.
    pub fn get_packages_by_platform(
        &self,
        platform: Platform,
    ) -> impl Iterator<Item = &'_ LockedDependency> + '_ {
        self.package
            .iter()
            .filter(move |pkg| pkg.platform == platform)
    }

    /// Returns all conda packages in the lock-file for a certain platform.
    pub fn get_conda_packages_by_platform(
        &self,
        platform: Platform,
    ) -> Result<Vec<RepoDataRecord>, ConversionError> {
        self.get_packages_by_platform(platform)
            .filter(|pkg| pkg.is_conda())
            .map(|pkg| pkg.try_into())
            .collect()
    }
}

#[cfg(test)]
mod test {
    use super::CondaLock;
    use crate::LockedDependency;
    use insta::assert_yaml_snapshot;
    use rattler_conda_types::{Platform, RepoDataRecord, VersionWithSource};
    use serde_yaml::from_str;
    use std::{path::Path, str::FromStr};

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

    #[test]
    fn test_locked_dependency() {
        let yaml = r#"
        name: ncurses
        version: '6.4'
        manager: conda
        platform: linux-64
        arch: x86_64
        dependencies:
            libgcc-ng: '>=12'
        url: https://conda.anaconda.org/conda-forge/linux-64/ncurses-6.4-hcb278e6_0.conda
        hash:
            md5: 681105bccc2a3f7f1a837d47d39c9179
            sha256: ccf61e61d58a8a7b2d66822d5568e2dc9387883dd9b2da61e1d787ece4c4979a
        optional: false
        category: main
        build: hcb278e6_0
        subdir: linux-64
        build_number: 0
        license: X11 AND BSD-3-Clause
        size: 880967
        timestamp: 1686076725450"#;

        let result: LockedDependency = from_str(yaml).unwrap();

        assert_eq!(result.name, "ncurses");
        assert_eq!(result.version.as_str(), "6.4");

        let repodata_record = RepoDataRecord::try_from(result.clone()).unwrap();

        assert_eq!(
            repodata_record.package_record.name.as_normalized(),
            "ncurses"
        );
        assert_eq!(
            repodata_record.package_record.version,
            VersionWithSource::from_str("6.4").unwrap()
        );
        assert!(repodata_record.package_record.noarch.is_none());

        insta::assert_yaml_snapshot!(repodata_record);
    }
}
