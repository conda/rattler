//! Defines [`RepoData`]. `RepoData` stores information of all packages present in a subdirectory
//! of a channel. It provides indexing functionality.

pub mod patches;
mod topological_sort;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::path::Path;

use fxhash::{FxHashMap, FxHashSet};

use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, OneOrMany};
use thiserror::Error;

use rattler_macros::sorted;

use crate::{
    package::IndexJson, utils::serde::DeserializeFromStrUnchecked, Channel, NoArchType,
    PackageName, Platform, RepoDataRecord, VersionWithSource,
};

/// [`RepoData`] is an index of package binaries available on in a subdirectory of a Conda channel.
// Note: we cannot use the sorted macro here, because the `packages` and `conda_packages` fields are
// serialized in a special way. Therefore we do it manually.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct RepoData {
    /// The channel information contained in the repodata.json file
    pub info: Option<ChannelInfo>,

    /// The tar.bz2 packages contained in the repodata.json file
    #[serde(serialize_with = "sort_map_alphabetically")]
    pub packages: FxHashMap<String, PackageRecord>,

    /// The conda packages contained in the repodata.json file (under a different key for
    /// backwards compatibility with previous conda versions)
    #[serde(rename = "packages.conda", serialize_with = "sort_map_alphabetically")]
    pub conda_packages: FxHashMap<String, PackageRecord>,

    /// removed packages (files are still accessible, but they are not installable like regular packages)
    #[serde(
        default,
        serialize_with = "sort_set_alphabetically",
        skip_serializing_if = "FxHashSet::is_empty"
    )]
    pub removed: FxHashSet<String>,

    /// The version of the repodata format
    #[serde(rename = "repodata_version")]
    pub version: Option<u64>,
}

/// Information about subdirectory of channel in the Conda [`RepoData`]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct ChannelInfo {
    /// The channel's subdirectory
    pub subdir: String,
}

/// A single record in the Conda repodata. A single record refers to a single binary distribution
/// of a package on a Conda channel.
#[serde_as]
#[skip_serializing_none]
#[sorted]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd, Clone, Hash)]
pub struct PackageRecord {
    /// Optionally the architecture the package supports
    pub arch: Option<String>,

    /// The build string of the package
    pub build: String,

    /// The build number of the package
    pub build_number: u64,

    /// Additional constraints on packages. `constrains` are different from `depends` in that packages
    /// specified in `depends` must be installed next to this package, whereas packages specified in
    /// `constrains` are not required to be installed, but if they are installed they must follow these
    /// constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constrains: Vec<String>,

    /// Specification of packages this package depends on
    #[serde(default)]
    pub depends: Vec<String>,

    /// Features are a deprecated way to specify different feature sets for the conda solver. This is not
    /// supported anymore and should not be used. Instead, `mutex` packages should be used to specify
    /// mutually exclusive features.
    pub features: Option<String>,

    /// A deprecated md5 hash
    pub legacy_bz2_md5: Option<String>,

    /// A deprecated package archive size.
    pub legacy_bz2_size: Option<u64>,

    /// The specific license of the package
    pub license: Option<String>,

    /// The license family
    pub license_family: Option<String>,

    /// Optionally a MD5 hash of the package archive
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,

    /// The name of the package
    #[serde_as(deserialize_as = "DeserializeFromStrUnchecked")]
    pub name: PackageName,

    /// If this package is independent of architecture this field specifies in what way. See
    /// [`NoArchType`] for more information.
    #[serde(skip_serializing_if = "NoArchType::is_none")]
    pub noarch: NoArchType,

    /// Optionally the platform the package supports
    pub platform: Option<String>, // Note that this does not match the [`Platform`] enum..

    /// Optionally a SHA256 hash of the package archive
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,

    /// Optionally the size of the package archive in bytes
    pub size: Option<u64>,

    /// The subdirectory where the package can be found
    #[serde(default)]
    pub subdir: String,

    /// The date this entry was created.
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Track features are nowadays only used to downweight packages (ie. give them less priority). To
    /// that effect, the number of track features is counted (number of commas) and the package is downweighted
    /// by the number of track_features.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "OneOrMany<_>")]
    pub track_features: Vec<String>,

    /// The version of the package
    pub version: VersionWithSource,
    // Looking at the `PackageRecord` class in the Conda source code a record can also include all
    // these fields. However, I have no idea if or how they are used so I left them out.
    //pub preferred_env: Option<String>,
    //pub date: Option<String>,
    //pub package_type: ?
}

impl Display for PackageRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.build.is_empty() {
            write!(f, "{} {}", self.name.as_normalized(), self.version,)
        } else {
            write!(
                f,
                "{}={}={}",
                self.name.as_normalized(),
                self.version,
                self.build
            )
        }
    }
}

impl RepoData {
    /// Parses [`RepoData`] from a file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let contents = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&contents)?)
    }

    /// Builds a [`Vec<RepoDataRecord>`] from the packages in a [`RepoData`] given the source of the
    /// data.
    pub fn into_repo_data_records(self, channel: &Channel) -> Vec<RepoDataRecord> {
        let mut records = Vec::with_capacity(self.packages.len() + self.conda_packages.len());
        let channel_name = channel.canonical_name();
        for (filename, package_record) in self.packages.into_iter().chain(self.conda_packages) {
            records.push(RepoDataRecord {
                url: channel
                    .base_url()
                    .join(&format!("{}/{}", &package_record.subdir, &filename))
                    .expect("failed to build a url from channel and package record"),
                channel: channel_name.clone(),
                package_record,
                file_name: filename,
            })
        }
        records
    }
}

impl PackageRecord {
    /// A simple helper method that constructs a `PackageRecord` with the bare minimum values.
    pub fn new(name: PackageName, version: impl Into<VersionWithSource>, build: String) -> Self {
        Self {
            arch: None,
            build,
            build_number: 0,
            constrains: vec![],
            depends: vec![],
            features: None,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            md5: None,
            name,
            noarch: Default::default(),
            platform: None,
            sha256: None,
            size: None,
            subdir: Platform::current().to_string(),
            timestamp: None,
            track_features: vec![],
            version: version.into(),
        }
    }

    /// Sorts the records topologically.
    ///
    /// This function is deterministic, meaning that it will return the same result regardless of
    /// the order of `records` and of the `depends` vector inside the records.
    ///
    /// Note that this function only works for packages with unique names.
    pub fn sort_topologically<T: AsRef<PackageRecord> + Clone>(records: Vec<T>) -> Vec<T> {
        topological_sort::sort_topologically(records)
    }
}

/// An error that can occur when parsing a platform from a string.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum ConvertSubdirError {
    /// No known combination for this platform is known
    #[error("platform: {platform}, arch: {arch} is not a known combination")]
    NoKnownCombination {
        /// The platform string that could not be parsed.
        platform: String,
        /// The architecture.
        arch: String,
    },
    /// Platform key is empty
    #[error("platform key is empty in index.json")]
    PlatformEmpty,
    /// Arc key is empty
    #[error("arch key is empty in index.json")]
    ArchEmpty,
}

/// Determine the subdir based on result taken from the prefix.dev
/// database
/// These were the combinations that have been found in the database.
/// and have been represented in the function.
///
/// # Why can we not use Platform::FromStr?
///
/// We cannot use the Platform FromStr directly because x86 and x86_64
/// are different architecture strings. Also some combinations have been removed,
/// because they have not been found.
fn determine_subdir(
    platform: Option<String>,
    arch: Option<String>,
) -> Result<String, ConvertSubdirError> {
    let platform = platform.ok_or(ConvertSubdirError::PlatformEmpty)?;
    let arch = arch.ok_or(ConvertSubdirError::ArchEmpty)?;

    let plat = match platform.as_ref() {
        "linux" => match arch.as_ref() {
            "x86" => Ok(Platform::Linux32),
            "x86_64" => Ok(Platform::Linux64),
            "aarch64" => Ok(Platform::LinuxAarch64),
            "armv61" => Ok(Platform::LinuxArmV6l),
            "armv71" => Ok(Platform::LinuxArmV7l),
            "ppc64le" => Ok(Platform::LinuxPpc64le),
            "ppc64" => Ok(Platform::LinuxPpc64),
            "s390x" => Ok(Platform::LinuxS390X),
            _ => Err(ConvertSubdirError::NoKnownCombination { platform, arch }),
        },
        "osx" => match arch.as_ref() {
            "x86_64" => Ok(Platform::Osx64),
            "arm64" => Ok(Platform::OsxArm64),
            _ => Err(ConvertSubdirError::NoKnownCombination { platform, arch }),
        },
        "windows" => match arch.as_ref() {
            "x86" => Ok(Platform::Win32),
            "x86_64" => Ok(Platform::Win64),
            "arm64" => Ok(Platform::WinArm64),
            _ => Err(ConvertSubdirError::NoKnownCombination { platform, arch }),
        },
        _ => Err(ConvertSubdirError::NoKnownCombination { platform, arch }),
    }?;
    // Convert back to Platform string which should correspond to known subdirs
    Ok(plat.to_string())
}

impl PackageRecord {
    /// Builds a [`PackageRecord`] from a [`IndexJson`] and optionally a size, sha256 and md5 hash.
    pub fn from_index_json(
        index: IndexJson,
        size: Option<u64>,
        sha256: Option<Sha256Hash>,
        md5: Option<Md5Hash>,
    ) -> Result<PackageRecord, ConvertSubdirError> {
        // Determine the subdir if it can't be found
        let subdir = match index.subdir {
            None => determine_subdir(index.platform.clone(), index.arch.clone())?,
            Some(s) => s,
        };

        Ok(PackageRecord {
            arch: index.arch,
            build: index.build,
            build_number: index.build_number,
            constrains: index.constrains,
            depends: index.depends,
            features: index.features,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: index.license,
            license_family: index.license_family,
            md5,
            name: index.name,
            noarch: index.noarch,
            platform: index.platform,
            sha256,
            size,
            subdir,
            timestamp: index.timestamp,
            track_features: index.track_features,
            version: index.version,
        })
    }
}

fn sort_map_alphabetically<T: Serialize, S: serde::Serializer>(
    value: &FxHashMap<String, T>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    return BTreeMap::from_iter(value.iter()).serialize(serializer);
}

fn sort_set_alphabetically<S: serde::Serializer>(
    value: &FxHashSet<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    return BTreeSet::from_iter(value.iter()).serialize(serializer);
}

#[cfg(test)]
mod test {
    use crate::repo_data::determine_subdir;
    use fxhash::FxHashSet;

    use crate::RepoData;

    // isl-0.12.2-1.tar.bz2
    // gmp-5.1.2-6.tar.bz2
    // Are both package variants in the osx-64 subdir
    // Will just test for this case
    #[test]
    fn test_determine_subdir() {
        assert_eq!(
            determine_subdir(Some("osx".to_string()), Some("x86_64".to_string())).unwrap(),
            "osx-64"
        );
    }

    #[test]
    fn test_serialize() {
        let repodata = RepoData {
            version: Some(1),
            info: Default::default(),
            packages: Default::default(),
            conda_packages: Default::default(),
            removed: FxHashSet::from_iter(
                ["xyz", "foo", "bar", "baz", "qux", "aux", "quux"]
                    .iter()
                    .map(|s| s.to_string()),
            ),
        };
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_serialize_packages() {
        // load test data
        let test_data_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
        let data_path = test_data_path.join("channels/dummy/linux-64/repodata.json");
        let repodata = RepoData::from_path(&data_path).unwrap();
        insta::assert_yaml_snapshot!(repodata);

        // serialize to json
        let json = serde_json::to_string_pretty(&repodata).unwrap();
        insta::assert_snapshot!(json);
    }
}
