//! A module that enables parsing of lock files version 3 or lower.

use std::{collections::BTreeSet, ops::Not, sync::Arc};

use fxhash::FxHashMap;
use indexmap::IndexSet;
use pep440_rs::VersionSpecifiers;
use pep508_rs::{ExtraName, Requirement};
use rattler_conda_types::{
    NoArchType, PackageName, PackageRecord, PackageUrl, Platform, VersionWithSource,
};
use serde::Deserialize;
use serde_with::{serde_as, skip_serializing_none, OneOrMany};
use url::Url;

use super::ParseCondaLockError;
use crate::{
    file_format_version::FileFormatVersion, Channel, CondaPackageData, EnvironmentData,
    EnvironmentPackageData, LockFile, LockFileInner, PackageHashes, PypiPackageData,
    PypiPackageEnvironmentData, UrlOrPath, DEFAULT_ENVIRONMENT_NAME,
};

#[derive(Deserialize)]
struct LockFileV3 {
    metadata: LockMetaV3,
    package: Vec<LockedPackageV3>,
}

#[serde_as]
#[derive(Deserialize, Clone, Debug, Eq, PartialEq)]
struct LockMetaV3 {
    /// Channels used to resolve dependencies
    pub channels: Vec<Channel>,
    /// The platforms this lock file supports
    #[serde_as(as = "crate::utils::serde::Ordered<_>")]
    pub platforms: Vec<Platform>,
}

#[derive(Deserialize, Eq, PartialEq, Clone, Debug)]
struct LockedPackageV3 {
    pub platform: Platform,
    #[serde(flatten)]
    pub kind: LockedPackageKindV3,
}

#[derive(Deserialize, Eq, PartialEq, Clone, Debug)]
#[serde(tag = "manager", rename_all = "snake_case")]
enum LockedPackageKindV3 {
    Conda(Box<CondaLockedPackageV3>),
    #[serde(alias = "pip")]
    Pypi(Box<PypiLockedPackageV3>),
}

#[serde_as]
#[skip_serializing_none]
#[derive(Deserialize, Eq, PartialEq, Clone, Debug)]
struct PypiLockedPackageV3 {
    pub name: String,
    pub version: pep440_rs::Version,
    #[serde(default, alias = "dependencies", skip_serializing_if = "Vec::is_empty")]
    #[serde_as(deserialize_as = "crate::utils::serde::Pep440MapOrVec")]
    pub requires_dist: Vec<Requirement>,
    pub requires_python: Option<VersionSpecifiers>,
    #[serde(flatten)]
    pub runtime: PypiPackageEnvironmentDataV3,
    pub url: Url,
    pub hash: Option<PackageHashes>,
    // These fields are not used by rattler-lock.
    // pub source: Option<Url>,
    // pub build: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Hash, Eq, PartialEq)]
pub struct PypiPackageEnvironmentDataV3 {
    #[serde(default)]
    pub extras: BTreeSet<ExtraName>,
}

impl From<PypiPackageEnvironmentDataV3> for PypiPackageEnvironmentData {
    fn from(config: PypiPackageEnvironmentDataV3) -> Self {
        Self {
            extras: config.extras.into_iter().collect(),
        }
    }
}

#[serde_as]
#[skip_serializing_none]
#[derive(Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct CondaLockedPackageV3 {
    pub name: String,
    pub version: VersionWithSource,
    #[serde(default)]
    #[serde_as(deserialize_as = "crate::utils::serde::MatchSpecMapOrVec")]
    pub dependencies: Vec<String>,
    pub url: Url,
    pub hash: PackageHashes,
    pub source: Option<Url>,
    #[serde(default)]
    pub build: String,
    pub arch: Option<String>,
    pub subdir: Option<String>,
    pub build_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constrains: Vec<String>,
    pub features: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "OneOrMany<_>")]
    pub track_features: Vec<String>,
    pub license: Option<String>,
    pub license_family: Option<String>,
    pub python_site_packages_path: Option<String>,
    #[serde(skip_serializing_if = "NoArchType::is_none")]
    pub noarch: NoArchType,
    pub size: Option<u64>,
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub purls: BTreeSet<PackageUrl>,
}

/// A function that enables parsing of lock files version 3 or lower.
pub fn parse_v3_or_lower(
    document: serde_yaml::Value,
    version: FileFormatVersion,
) -> Result<LockFile, ParseCondaLockError> {
    let lock_file: LockFileV3 =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;

    // Iterate over all packages, deduplicate them and store the list of packages
    // per platform. There might be duplicates for noarch packages.
    let mut conda_packages = IndexSet::with_capacity(lock_file.package.len());
    let mut pypi_packages = IndexSet::with_capacity(lock_file.package.len());
    let mut pypi_runtime_configs = IndexSet::with_capacity(lock_file.package.len());
    let mut per_platform: FxHashMap<Platform, Vec<EnvironmentPackageData>> = FxHashMap::default();
    for package in lock_file.package {
        let LockedPackageV3 { platform, kind } = package;

        let pkg: EnvironmentPackageData = match kind {
            LockedPackageKindV3::Conda(value) => {
                let md5 = match value.hash {
                    PackageHashes::Md5(md5) | PackageHashes::Md5Sha256(md5, _) => Some(md5),
                    PackageHashes::Sha256(_) => None,
                };
                let sha256 = match value.hash {
                    PackageHashes::Sha256(sha256) | PackageHashes::Md5Sha256(_, sha256) => {
                        Some(sha256)
                    }
                    PackageHashes::Md5(_) => None,
                };

                let deduplicated_idx = conda_packages
                    .insert_full(CondaPackageData {
                        package_record: PackageRecord {
                            arch: value.arch,
                            build: value.build,
                            build_number: value.build_number.unwrap_or(0),
                            constrains: value.constrains,
                            depends: value.dependencies,
                            features: value.features,
                            legacy_bz2_md5: None,
                            legacy_bz2_size: None,
                            license: value.license,
                            license_family: value.license_family,
                            md5,
                            name: PackageName::new_unchecked(value.name),
                            noarch: value.noarch,
                            platform: platform.only_platform().map(ToString::to_string),
                            sha256,
                            size: value.size,
                            subdir: value.subdir.unwrap_or(platform.to_string()),
                            timestamp: value.timestamp,
                            track_features: value.track_features,
                            version: value.version,
                            purls: value.purls.is_empty().not().then_some(value.purls),
                            python_site_packages_path: value.python_site_packages_path,
                            run_exports: None,
                        },
                        url: value.url,
                        file_name: None,
                        channel: None,
                    })
                    .0;

                EnvironmentPackageData::Conda(deduplicated_idx)
            }
            LockedPackageKindV3::Pypi(pkg) => {
                let deduplicated_index = pypi_packages
                    .insert_full(PypiPackageData {
                        name: pep508_rs::PackageName::new(pkg.name)?,
                        version: pkg.version,
                        requires_dist: pkg.requires_dist,
                        requires_python: pkg.requires_python,
                        url_or_path: UrlOrPath::Url(pkg.url),
                        hash: pkg.hash,
                        editable: false,
                    })
                    .0;
                EnvironmentPackageData::Pypi(
                    deduplicated_index,
                    pypi_runtime_configs.insert_full(pkg.runtime).0,
                )
            }
        };

        per_platform.entry(package.platform).or_default().push(pkg);
    }

    // Construct the default environment
    let default_environment = EnvironmentData {
        channels: lock_file.metadata.channels,
        indexes: None,
        packages: per_platform,
    };

    Ok(LockFile {
        inner: Arc::new(LockFileInner {
            version,
            conda_packages: conda_packages.into_iter().collect(),
            pypi_packages: pypi_packages.into_iter().collect(),
            pypi_environment_package_data: pypi_runtime_configs
                .into_iter()
                .map(Into::into)
                .collect(),

            environment_lookup: [(DEFAULT_ENVIRONMENT_NAME.to_string(), 0)]
                .into_iter()
                .collect(),
            environments: vec![default_environment],
        }),
    })
}
