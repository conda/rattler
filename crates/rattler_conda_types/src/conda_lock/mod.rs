//! This is the definitions for a conda-lock file format
//! It is modeled on the definitions found at: [conda-lock models](https://github.com/conda/conda-lock/blob/main/conda_lock/lockfile/models.py)
//! Most names were kept the same as in the models file. So you can refer to those exactly.
//! However, some types were added to enforce a bit more type safety.
use self::PackageHashes::{Md5, Md5Sha256, Sha256};
use crate::match_spec::parse::ParseMatchSpecError;
use crate::{
    utils::serde::Ordered, NamelessMatchSpec, NoArchType, PackageRecord, ParsePlatformError,
    ParseVersionError, Platform, RepoDataRecord,
};
use crate::{MatchSpec, PackageName};
use fxhash::FxBuildHasher;
use indexmap::IndexMap;
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{serde_as, skip_serializing_none, DisplayFromStr};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path, str::FromStr};
use url::Url;

pub mod builder;
mod content_hash;

/// Represents the conda-lock file
/// Contains the metadata regarding the lock files
/// also the locked packages
#[derive(Deserialize, Clone, Debug)]
pub struct CondaLock {
    /// Metadata for the lock file
    pub metadata: LockMeta,

    /// Locked packages
    pub package: Vec<LockedDependency>,
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

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
/// Metadata for the [`CondaLock`] file
pub struct LockMeta {
    /// Hash of dependencies for each target platform
    pub content_hash: BTreeMap<Platform, String>,
    /// Channels used to resolve dependencies
    pub channels: Vec<Channel>,
    /// The platforms this lock file supports
    #[serde_as(as = "Ordered<_>")]
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

/// Represents whether this is a dependency managed by pip or conda
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Manager {
    /// The "conda" manager
    Conda,
    /// The pip manager
    Pip,
}

/// This implementation of the `Deserialize` trait for the `PackageHashes` struct
///
/// It expects the input to have either a `md5` field, a `sha256` field, or both.
/// If both fields are present, it constructs a `Md5Sha256` instance with their values.
/// If only the `md5` field is present, it constructs a `Md5` instance with its value.
/// If only the `sha256` field is present, it constructs a `Sha256` instance with its value.
/// If neither field is present it returns an error
#[derive(Eq, PartialEq, Hash, Clone, Debug)]
pub enum PackageHashes {
    /// Contains an MD5 hash
    Md5(Md5Hash),
    /// Contains as Sha256 Hash
    Sha256(Sha256Hash),
    /// Contains both hashes
    Md5Sha256(Md5Hash, Sha256Hash),
}

impl PackageHashes {
    /// Create correct enum from hashes
    pub fn from_hashes(md5: Option<Md5Hash>, sha256: Option<Sha256Hash>) -> Option<PackageHashes> {
        match (md5, sha256) {
            (Some(md5), None) => Some(Md5(md5)),
            (None, Some(sha256)) => Some(Sha256(sha256)),
            (Some(md5), Some(sha256)) => Some(Md5Sha256(md5, sha256)),
            (None, None) => None,
        }
    }
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
#[serde_as]
#[skip_serializing_none]
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct LockedDependency {
    /// Package name of dependency
    pub name: PackageName,
    /// Locked version
    pub version: String,
    /// Pip or Conda managed
    pub manager: Manager,
    /// What platform is this package for (different to other places in the conda ecosystem,
    /// this actually represents the _full_ subdir (incl. arch))
    pub platform: Platform,
    /// What are its own dependencies mapping name to version constraint
    #[serde_as(as = "IndexMap<_, DisplayFromStr, FxBuildHasher>")]
    pub dependencies: IndexMap<PackageName, NamelessMatchSpec, FxBuildHasher>,
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

    /// Experimental: architecture field
    pub arch: Option<String>,

    /// Experimental: the subdir where the package can be found
    pub subdir: Option<String>,

    /// Experimental: conda build number of the package
    pub build_number: Option<u64>,

    /// Experimental: see: [Constrains](crate::repo_data::PackageRecord::constrains)
    pub constrains: Option<Vec<String>>,

    /// Experimental: see: [Features](crate::repo_data::PackageRecord::features)
    pub features: Option<String>,

    /// Experimental: see: [Track features](crate::repo_data::PackageRecord::track_features)
    pub track_features: Option<Vec<String>>,

    /// Experimental: the specific license of the package
    pub license: Option<String>,

    /// Experimental: the license family of the package
    pub license_family: Option<String>,

    /// Experimental: If this package is independent of architecture this field specifies in what way. See
    /// [`NoArchType`] for more information.
    #[serde(skip_serializing_if = "NoArchType::is_none")]
    pub noarch: NoArchType,

    /// Experimental: The size of the package archive in bytes
    pub size: Option<u64>,

    /// Experimental: The date this entry was created.
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Error used when converting from repo_data module to conda lock module
#[derive(thiserror::Error, Debug)]
pub enum ConversionError {
    /// This field was found missing during the conversion
    #[error("missing field/fields '{0}'")]
    Missing(String),
    /// Parse error when converting [`MatchSpec`]
    #[error(transparent)]
    MatchSpecConversion(#[from] ParseMatchSpecError),
    /// Error when version parsing fails
    #[error(transparent)]
    VersionConversion(#[from] ParseVersionError),
}

/// Package filename from the url
fn file_name_from_url(url: &Url) -> Option<&str> {
    let path = url.path_segments()?;
    path.last()
}

/// Channel from url, this is everything before the filename and the subdir
/// So for example: https://conda.anaconda.org/conda-forge/ is a channel name
/// that we parse from something like: https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda
fn channel_from_url(url: &Url) -> Option<Url> {
    let mut result = url.clone();

    // Strip the last two path segments. We assume the first one contains the file_name, and the
    // other the subdirectory.
    result.path_segments_mut().ok()?.pop().pop();

    Some(result)
}

impl TryFrom<&LockedDependency> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: &LockedDependency) -> Result<Self, Self::Error> {
        Self::try_from(value.clone())
    }
}

impl TryFrom<LockedDependency> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: LockedDependency) -> Result<Self, Self::Error> {
        let matchspecs = value
            .dependencies
            .into_iter()
            .map(|(name, matchspec)| MatchSpec::from_nameless(matchspec, Some(name)).to_string())
            .collect::<Vec<_>>();

        let version = value.version.parse()?;
        let md5 = match value.hash {
            Md5(md5) => Some(md5),
            Md5Sha256(md5, _) => Some(md5),
            _ => None,
        };
        let sha256 = match value.hash {
            Sha256(sha256) => Some(sha256),
            Md5Sha256(_, sha256) => Some(sha256),
            _ => None,
        };
        let channel = channel_from_url(&value.url)
            .ok_or_else(|| ConversionError::Missing("channel in url".to_string()))?
            .to_string();
        let file_name = file_name_from_url(&value.url)
            .ok_or_else(|| ConversionError::Missing("filename in url".to_string()))?
            .to_owned();
        let build = value
            .build
            .ok_or_else(|| ConversionError::Missing("build".to_string()))?;

        let platform = value.platform.only_platform();

        Ok(Self {
            package_record: PackageRecord {
                arch: value.arch,
                build,
                build_number: value.build_number.unwrap_or(0),
                constrains: value.constrains.unwrap_or_default(),
                depends: matchspecs,
                features: value.features,
                legacy_bz2_md5: None,
                legacy_bz2_size: None,
                license: value.license,
                license_family: value.license_family,
                md5,
                name: value.name,
                noarch: value.noarch,
                platform: platform.map(|p| p.to_string()),
                sha256,
                size: value.size,
                subdir: value.subdir.unwrap_or(value.platform.to_string()),
                timestamp: value.timestamp,
                track_features: value.track_features.unwrap_or_default(),
                version,
            },
            file_name,
            url: value.url,
            channel,
        })
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
    #[serde_as(as = "Ordered<_>")]
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

impl Serialize for CondaLock {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Raw<'a> {
            metadata: &'a LockMeta,
            package: Vec<&'a LockedDependency>,
            version: u32,
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
                .then_with(|| a.build.cmp(&b.build))
        });

        let raw = Raw {
            metadata: &self.metadata,
            package: sorted_deps,
            version: 1,
        };

        raw.serialize(serializer)
    }
}

#[cfg(test)]
mod test {
    use super::{channel_from_url, file_name_from_url, CondaLock, PackageHashes};
    use crate::{Platform, RepoDataRecord, VersionWithSource};
    use insta::assert_yaml_snapshot;
    use serde_yaml::from_str;
    use std::{path::Path, str::FromStr};
    use url::Url;

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

    #[test]
    fn test_channel_from_url() {
        assert_eq!(channel_from_url(&Url::parse("https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda").unwrap()), Some(Url::parse("https://conda.anaconda.org/conda-forge").unwrap()));
        assert_eq!(
            channel_from_url(
                &Url::parse(
                    "file:///C:/Users/someone/AppData/Local/Temp/.tmpsasJ7b/noarch/foo-1-0.conda"
                )
                .unwrap()
            ),
            Some(Url::parse("file:///C:/Users/someone/AppData/Local/Temp/.tmpsasJ7b").unwrap())
        );
    }

    #[test]
    fn test_file_name_from_url() {
        assert_eq!(file_name_from_url(&Url::parse("https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda").unwrap()), Some("python-3.11.0-h4150a38_1_cpython.conda"));
        assert_eq!(
            file_name_from_url(
                &Url::parse(
                    "file:///C:/Users/someone/AppData/Local/Temp/.tmpsasJ7b/noarch/foo-1-0.conda"
                )
                .unwrap()
            ),
            Some("foo-1-0.conda")
        );
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

        let result: crate::conda_lock::LockedDependency = from_str(yaml).unwrap();

        assert_eq!(result.name.as_normalized(), "ncurses");
        assert_eq!(result.version, "6.4");

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
