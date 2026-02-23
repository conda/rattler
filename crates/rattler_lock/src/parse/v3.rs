//! A module that enables parsing of lock files version 3 or lower.

use std::{collections::BTreeSet, ops::Not, str::FromStr, sync::Arc};

use super::{
    models::legacy::{LegacyCondaBinaryData, LegacyCondaPackageData},
    ParseCondaLockError,
};
use crate::{
    file_format_version::FileFormatVersion,
    platform::PlatformName,
    utils::derived_fields::{
        derive_arch_and_platform, derive_build_number_from_build, derive_noarch_type,
        LocationDerivedFields,
    },
    Channel, CondaPackageData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    PackageHashes, PlatformData, PypiPackageData, SolveOptions, UrlOrPath, Verbatim,
    DEFAULT_ENVIRONMENT_NAME,
};
use indexmap::IndexSet;
use pep440_rs::VersionSpecifiers;
use pep508_rs::Requirement;
use rattler_conda_types::{NoArchType, PackageName, PackageRecord, PackageUrl, VersionWithSource};
use serde::Deserialize;
use serde_with::{serde_as, skip_serializing_none, OneOrMany};
use url::Url;

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
    pub platforms: Vec<rattler_conda_types::Platform>,
}

#[derive(Deserialize, Eq, PartialEq, Clone, Debug)]
struct LockedPackageV3 {
    pub platform: rattler_conda_types::Platform,
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
    pub url: Url,
    pub hash: Option<PackageHashes>,
    // These fields are not used by rattler-lock.
    // pub source: Option<Url>,
    // pub build: Option<String>,
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
    pub build: Option<String>,
    pub arch: Option<String>,
    // Platform is used to indicate the actual platform to which this package belongs.
    // pub platform: Option<String>,
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
    pub noarch: Option<NoArchType>,
    pub python_site_packages_path: Option<String>,
    pub size: Option<u64>,
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub purls: BTreeSet<PackageUrl>,
}

fn create_platforms_and_packages(
    mut per_package: ahash::HashMap<
        rattler_conda_types::Platform,
        IndexSet<EnvironmentPackageData>,
    >,
) -> (
    Vec<PlatformData>,
    ahash::HashMap<usize, IndexSet<EnvironmentPackageData>>,
) {
    let mut unique_platforms = ahash::HashSet::default();

    let mut platforms = per_package
        .keys()
        .filter(|platform| unique_platforms.insert(**platform))
        .map(|platform| PlatformData {
            name: PlatformName::try_from(platform.to_string())
                .expect("All `rattler_conda_types::Platform` are valid platform names"),
            subdir: *platform,
            virtual_packages: Vec::new(),
        })
        .collect::<Vec<_>>();

    platforms.sort_by_key(|p| p.name.to_string());

    let packages = per_package
        .drain()
        .map(|(k, v)| {
            (
                platforms
                    .iter()
                    .position(|p| p.name.as_str() == k.as_str())
                    .expect("All Platforms in this hashmap were added before"),
                v,
            )
        })
        .collect();
    (platforms, packages)
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
    let mut per_platform: ahash::HashMap<
        rattler_conda_types::Platform,
        IndexSet<EnvironmentPackageData>,
    > = ahash::HashMap::default();
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

                // Guess the subdir from the url.
                let subdir = value
                    .subdir
                    .as_deref()
                    .or_else(|| {
                        value
                            .url
                            .path_segments()
                            .and_then(|split| split.rev().nth(1))
                    })
                    .and_then(|subdir_str| rattler_conda_types::Platform::from_str(subdir_str).ok())
                    .unwrap_or(platform);

                let location = UrlOrPath::Url(value.url).normalize().into_owned();
                let derived = LocationDerivedFields::new(&location);

                let build = value
                    .build
                    .or_else(|| derived.build.clone())
                    .unwrap_or_default();
                let build_number = value
                    .build_number
                    .or_else(|| derive_build_number_from_build(&build))
                    .unwrap_or(0);
                let derived_noarch = derive_noarch_type(
                    derived.subdir.as_deref().unwrap_or(subdir.as_str()),
                    derived.build.as_deref().unwrap_or(&build),
                );
                let (derived_arch, derived_platform) =
                    derive_arch_and_platform(derived.subdir.as_deref().unwrap_or(subdir.as_str()));

                let deduplicated_idx = conda_packages
                    .insert_full(LegacyCondaPackageData::Binary(LegacyCondaBinaryData {
                        channel: derived.channel,
                        file_name: derived.identifier.map_or_else(
                            || format!("{}-{}-{}.conda", value.name, value.version, build),
                            |id| id.to_file_name(),
                        ),
                        package_record: PackageRecord {
                            arch: value.arch.or(derived_arch),
                            build,
                            build_number,
                            constrains: value.constrains,
                            depends: value.dependencies,
                            experimental_extra_depends: std::collections::BTreeMap::new(),
                            features: value.features,
                            legacy_bz2_md5: None,
                            legacy_bz2_size: None,
                            license: value.license,
                            license_family: value.license_family,
                            md5,
                            name: PackageName::new_unchecked(value.name),
                            noarch: value.noarch.unwrap_or(derived_noarch),
                            platform: derived_platform,
                            sha256,
                            size: value.size,
                            subdir: subdir.to_string(),
                            timestamp: value.timestamp.map(Into::into),
                            track_features: value.track_features,
                            version: value.version,
                            purls: value.purls.is_empty().not().then_some(value.purls),
                            python_site_packages_path: value.python_site_packages_path,
                            run_exports: None,
                        },
                        location,
                    }))
                    .0;

                EnvironmentPackageData::Conda(deduplicated_idx)
            }
            LockedPackageKindV3::Pypi(pkg) => {
                let deduplicated_index = pypi_packages
                    .insert_full(PypiPackageData {
                        name: pep508_rs::PackageName::new(pkg.name)?,
                        version: Some(pkg.version),
                        requires_dist: pkg.requires_dist,
                        requires_python: pkg.requires_python,
                        location: Verbatim::new(UrlOrPath::Url(pkg.url)),
                        hash: pkg.hash,
                    })
                    .0;
                EnvironmentPackageData::Pypi(deduplicated_index)
            }
        };

        per_platform
            .entry(package.platform)
            .or_default()
            .insert(pkg);
    }

    let (platforms, packages) = create_platforms_and_packages(per_platform);
    // Construct the default environment
    let default_environment = EnvironmentData {
        channels: lock_file.metadata.channels,
        indexes: None,
        packages,
        options: SolveOptions::default(),
    };

    Ok(LockFile {
        inner: Arc::new(LockFileInner {
            version,
            platforms,
            conda_packages: conda_packages
                .into_iter()
                .map(CondaPackageData::from)
                .collect(),
            pypi_packages: pypi_packages.into_iter().collect(),
            environment_lookup: [(DEFAULT_ENVIRONMENT_NAME.to_string(), 0)]
                .into_iter()
                .collect(),
            environments: vec![default_environment],
        }),
    })
}
