use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    path::Path,
    sync::Arc,
};

use indexmap::IndexSet;
use pep440_rs::VersionSpecifiers;
use pep508_rs::ExtraName;
use rattler_conda_types::{PackageName, Platform, VersionWithSource};
use serde::{de::Error, Deserialize, Deserializer};
use serde_with::{serde_as, DeserializeAs};
use serde_yaml::Value;

use crate::{
    file_format_version::FileFormatVersion,
    parse::{
        models::{self, legacy::LegacyCondaPackageData, v6, v7},
        V5, V6, V7,
    },
    Channel, CondaPackageData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    PackageHashes, ParseCondaLockError, PypiIndexes, PypiPackageData, PypiPackageEnvironmentData,
    SolveOptions, UrlOrPath, Verbatim,
};

#[serde_as]
#[derive(Deserialize)]
#[serde(bound(deserialize = "P: DeserializeAs<'de, PackageData>"))]
struct DeserializableLockFile<P> {
    environments: BTreeMap<String, DeserializableEnvironment>,
    #[serde_as(as = "Vec<P>")]
    packages: Vec<PackageData>,
    #[serde(skip)]
    _data: PhantomData<P>,
}

/// Lock file struct for legacy (V4-V6) deserialization.
///
/// Uses `LegacyPackageData` instead of `PackageData` to preserve intermediate
/// types needed for package disambiguation before converting to final types.
#[serde_as]
#[derive(Deserialize)]
#[serde(bound(deserialize = "P: DeserializeAs<'de, LegacyPackageData>"))]
struct DeserializableLockFileLegacy<P> {
    environments: BTreeMap<String, DeserializableEnvironment>,
    #[serde_as(as = "Vec<P>")]
    packages: Vec<LegacyPackageData>,
    #[serde(skip)]
    _data: PhantomData<P>,
}

/// A pinned Pypi package
#[derive(Eq, PartialEq, Clone, Debug, Hash)]
pub struct PypiPackageDataRaw {
    /// The name of the package.
    pub name: pep508_rs::PackageName,

    /// The version of the package.
    pub version: pep440_rs::Version,

    /// The location of the package. This can be a URL or a path.
    pub location: Verbatim<UrlOrPath>,

    /// Hashes of the file pointed to by `url`.
    pub hash: Option<PackageHashes>,

    /// A list of (unparsed!) dependencies on other packages.
    pub requires_dist: Vec<String>,

    /// The python version that this package requires.
    pub requires_python: Option<VersionSpecifiers>,
}

impl From<PypiPackageData> for PypiPackageDataRaw {
    fn from(value: PypiPackageData) -> Self {
        let requires_dist = value
            .requires_dist
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>();

        Self {
            name: value.name.clone(),
            version: value.version.clone(),
            location: value.location.clone(),
            hash: value.hash.clone(),
            requires_dist,
            requires_python: value.requires_python.clone(),
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum PackageData {
    Conda(CondaPackageData),
    Pypi(PypiPackageDataRaw),
}

/// Package data enum used during legacy (V4-V6) deserialization.
///
/// This intermediate type preserves all fields needed for package disambiguation
/// before converting to the final `CondaPackageData` type.
#[allow(clippy::large_enum_variant)]
enum LegacyPackageData {
    Conda(LegacyCondaPackageData),
    Pypi(PypiPackageDataRaw),
}

#[derive(Deserialize)]
struct DeserializableEnvironment {
    channels: Vec<Channel>,
    #[serde(flatten)]
    indexes: Option<PypiIndexes>,
    #[serde(default)]
    options: SolveOptions,
    packages: BTreeMap<Platform, Vec<DeserializablePackageSelector>>,
}

impl<'de> DeserializeAs<'de, LegacyPackageData> for V5 {
    fn deserialize_as<D>(deserializer: D) -> Result<LegacyPackageData, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        #[allow(clippy::large_enum_variant)]
        enum Inner<'d> {
            Conda(models::v5::CondaPackageDataModel<'d>),
            Pypi(models::v5::PypiPackageDataModel<'d>),
        }

        Ok(match Inner::deserialize(deserializer)? {
            Inner::Conda(c) => LegacyPackageData::Conda(c.into()),
            Inner::Pypi(p) => LegacyPackageData::Pypi(p.into()),
        })
    }
}

impl<'de> DeserializeAs<'de, LegacyPackageData> for V6 {
    fn deserialize_as<D>(deserializer: D) -> Result<LegacyPackageData, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Discriminant {
            Conda {
                #[serde(rename = "conda")]
                _conda: String,
            },
            Pypi {
                #[serde(rename = "pypi")]
                _pypi: String,
            },
        }

        let value = serde_value::Value::deserialize(deserializer)?;
        let Ok(discriminant) = Discriminant::deserialize(
            serde_value::ValueDeserializer::<D::Error>::new(value.clone()),
        ) else {
            return Err(D::Error::custom(
                "expected at least `conda` or `pypi` field",
            ));
        };

        let deserializer = serde_value::ValueDeserializer::<D::Error>::new(value);
        Ok(match discriminant {
            Discriminant::Conda { .. } => LegacyPackageData::Conda(
                v6::CondaPackageDataModel::deserialize(deserializer)?
                    .try_into()
                    .map_err(D::Error::custom)?,
            ),
            Discriminant::Pypi { .. } => {
                LegacyPackageData::Pypi(v6::PypiPackageDataModel::deserialize(deserializer)?.into())
            }
        })
    }
}

impl<'de> DeserializeAs<'de, PackageData> for V7 {
    fn deserialize_as<D>(deserializer: D) -> Result<PackageData, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Discriminant {
            Conda {
                #[serde(rename = "conda")]
                _conda: String,
            },
            Pypi {
                #[serde(rename = "pypi")]
                _pypi: String,
            },
        }

        let value = serde_value::Value::deserialize(deserializer)?;
        let Ok(discriminant) = Discriminant::deserialize(
            serde_value::ValueDeserializer::<D::Error>::new(value.clone()),
        ) else {
            return Err(D::Error::custom(
                "expected at least `conda` or `pypi` field",
            ));
        };

        let deserializer = serde_value::ValueDeserializer::<D::Error>::new(value);
        Ok(match discriminant {
            Discriminant::Conda { .. } => PackageData::Conda(
                v7::CondaPackageDataModel::deserialize(deserializer)?
                    .try_into()
                    .map_err(D::Error::custom)?,
            ),
            Discriminant::Pypi { .. } => {
                PackageData::Pypi(v7::PypiPackageDataModel::deserialize(deserializer)?.into())
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
enum DeserializablePackageSelector {
    Conda {
        conda: UrlOrPath,
        name: Option<PackageName>,
        version: Option<VersionWithSource>,
        build: Option<String>,
        subdir: Option<String>,
    },
    Pypi {
        pypi: Verbatim<UrlOrPath>,
        #[serde(flatten)]
        runtime: DeserializablePypiPackageEnvironmentData,
    },
}

#[derive(Hash, Deserialize, Eq, PartialEq)]
struct DeserializablePypiPackageEnvironmentData {
    #[serde(default)]
    extras: BTreeSet<ExtraName>,
}

impl From<DeserializablePypiPackageEnvironmentData> for PypiPackageEnvironmentData {
    fn from(config: DeserializablePypiPackageEnvironmentData) -> Self {
        Self {
            extras: config.extras.into_iter().collect(),
        }
    }
}

/// Parses a [`LockFile`] from a [`serde_yaml::Value`] for V4/V5 format.
pub fn parse_from_document_v5(
    document: Value,
    version: FileFormatVersion,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFileLegacy<V5> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock_legacy(version, raw, None)
}

/// Parses a [`LockFile`] from a [`serde_yaml::Value`] for V6 format.
pub fn parse_from_document_v6(
    document: Value,
    base_dir: Option<&Path>,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFileLegacy<V6> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock_legacy(FileFormatVersion::V6, raw, base_dir)
}

pub fn parse_from_document_v7(
    document: Value,
    base_dir: Option<&Path>,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFile<V7> =
        serde_yaml::from_value::<DeserializableLockFile<V7>>(document)
            .map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock(FileFormatVersion::V7, raw, base_dir)
}

fn convert_raw_pypi_package(
    file_version: FileFormatVersion,
    raw_package: PypiPackageDataRaw,
    base_dir: Option<&Path>,
) -> Result<PypiPackageData, ParseCondaLockError> {
    let requires_dist = raw_package
        .requires_dist
        .iter()
        .map(|input| {
            if let Some(base_dir) = base_dir {
                pep508_rs::Requirement::parse(input, base_dir)
            } else {
                use std::str::FromStr as _;

                pep508_rs::Requirement::from_str(input)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    let location = if file_version < FileFormatVersion::V7 {
        Verbatim::new(raw_package.location.take())
    } else {
        raw_package.location
    };

    Ok(PypiPackageData {
        name: raw_package.name,
        version: raw_package.version,
        location,
        hash: raw_package.hash,
        requires_dist,
        requires_python: raw_package.requires_python,
    })
}

/// Checks if a legacy conda package matches the given selector fields.
fn legacy_package_matches_selector(
    package: &LegacyCondaPackageData,
    name: Option<&PackageName>,
    version: Option<&VersionWithSource>,
    build: Option<&str>,
    subdir: Option<&str>,
) -> bool {
    let record = package.record();
    name.is_none_or(|n| n == &record.name)
        && version.is_none_or(|v| v == &record.version)
        && build.is_none_or(|b| b == record.build)
        && subdir.is_none_or(|s| s == record.subdir)
}

/// Resolves a conda package index for legacy lock file formats (V1-V6).
///
/// For V6, uses exact field matching (name, version, build, subdir).
/// For V5 and earlier, matches by platform/subdir and merges duplicate packages
/// that share the same URL to combine optional fields like purls and hashes.
#[allow(clippy::too_many_arguments)]
fn resolve_legacy_conda_package(
    file_version: FileFormatVersion,
    platform: Platform,
    name: Option<&PackageName>,
    version: Option<&VersionWithSource>,
    build: Option<&str>,
    subdir: Option<&str>,
    candidate_indices: &[usize],
    packages: &mut Vec<LegacyCondaPackageData>,
) -> Option<usize> {
    if file_version >= FileFormatVersion::V6 {
        // V6+: Find the package that matches all selector fields exactly.
        return candidate_indices
            .iter()
            .find(|&&idx| {
                legacy_package_matches_selector(&packages[idx], name, version, build, subdir)
            })
            .copied();
    }

    // V5 and earlier: Match by platform subdir and merge duplicates.
    // This handles a historical quirk where multiple packages could share
    // the same URL but have different metadata.
    let packages_for_platform: Vec<_> = candidate_indices
        .iter()
        .filter(|&&idx| packages[idx].record().subdir.as_str() == platform.as_str())
        .copied()
        .collect();

    // Fall back to all candidates if none match the platform
    let matching_indices = if packages_for_platform.is_empty() {
        candidate_indices
    } else {
        &packages_for_platform
    };

    let mut iter = matching_indices.iter().copied();
    let first_idx = iter.next()?;

    // Merge all matching packages into one
    let merged = iter.fold(
        Cow::Borrowed(&packages[first_idx]),
        |acc, next_idx| match acc.merge(&packages[next_idx]) {
            Cow::Owned(merged) => Cow::Owned(merged),
            Cow::Borrowed(_) => acc,
        },
    );

    Some(match merged {
        Cow::Borrowed(_) => first_idx,
        Cow::Owned(merged_package) => {
            packages.push(merged_package);
            packages.len() - 1
        }
    })
}

/// Parses a legacy lock file (V4-V6) using `LegacyCondaPackageData` as the
/// intermediate type, then converts to `CondaPackageData` at the end.
///
/// This separation allows `CondaPackageData` to evolve for newer versions (V7+)
/// without breaking backward compatibility with older lock files.
fn parse_from_lock_legacy<P>(
    file_version: FileFormatVersion,
    raw: DeserializableLockFileLegacy<P>,
    base_dir: Option<&Path>,
) -> Result<LockFile, ParseCondaLockError> {
    // Split the packages into conda and pypi packages using legacy intermediate types.
    let mut legacy_conda_packages: Vec<LegacyCondaPackageData> = Vec::new();
    let mut pypi_packages = Vec::new();

    for package in raw.packages {
        match package {
            LegacyPackageData::Conda(p) => legacy_conda_packages.push(p),
            LegacyPackageData::Pypi(p) => {
                pypi_packages.push(convert_raw_pypi_package(file_version, p, base_dir)?);
            }
        }
    }

    // Determine the indices of the packages by url
    let mut conda_url_lookup: ahash::HashMap<UrlOrPath, Vec<_>> = ahash::HashMap::default();
    for (idx, conda_package) in legacy_conda_packages.iter().enumerate() {
        conda_url_lookup
            .entry(conda_package.location().clone())
            .or_default()
            .push(idx);
    }

    let pypi_url_lookup = pypi_packages
        .iter()
        .enumerate()
        .map(|(idx, p)| (&p.location, idx))
        .collect::<ahash::HashMap<_, _>>();
    let mut pypi_runtime_lookup = IndexSet::new();

    let environments = raw
        .environments
        .into_iter()
        .map(|(environment_name, env)| {
            Ok((
                environment_name.clone(),
                EnvironmentData {
                    channels: env.channels,
                    indexes: env.indexes,
                    options: env.options,
                    packages: env
                        .packages
                        .into_iter()
                        .map(|(platform, packages)| {
                            let packages = packages
                                .into_iter()
                                .map(|p| {
                                    Ok(match p {
                                        DeserializablePackageSelector::Conda {
                                            conda,
                                            name,
                                            version,
                                            build,
                                            subdir,
                                        } => {
                                            let candidate_indices = conda_url_lookup
                                                .get(&conda)
                                                .map_or(&[] as &[usize], Vec::as_slice);

                                            let package_index = resolve_legacy_conda_package(
                                                file_version,
                                                platform,
                                                name.as_ref(),
                                                version.as_ref(),
                                                build.as_deref(),
                                                subdir.as_deref(),
                                                candidate_indices,
                                                &mut legacy_conda_packages,
                                            );

                                            EnvironmentPackageData::Conda(
                                                package_index.ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        environment_name.clone(),
                                                        platform,
                                                        conda,
                                                    )
                                                })?,
                                            )
                                        }
                                        DeserializablePackageSelector::Pypi { pypi, runtime } => {
                                            EnvironmentPackageData::Pypi(
                                                *pypi_url_lookup.get(&pypi).ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        environment_name.clone(),
                                                        platform,
                                                        pypi.inner().clone(),
                                                    )
                                                })?,
                                                pypi_runtime_lookup.insert_full(runtime).0,
                                            )
                                        }
                                    })
                                })
                                .collect::<Result<_, ParseCondaLockError>>()?;

                            Ok((platform, packages))
                        })
                        .collect::<Result<_, ParseCondaLockError>>()?,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, ParseCondaLockError>>()?;

    let (environment_lookup, environments) = environments
        .into_iter()
        .enumerate()
        .map(|(idx, (name, env))| ((name, idx), env))
        .unzip();

    // Convert legacy conda packages to final CondaPackageData
    let conda_packages: Vec<CondaPackageData> =
        legacy_conda_packages.into_iter().map(Into::into).collect();

    Ok(LockFile {
        inner: Arc::new(LockFileInner {
            version: file_version,
            environments,
            environment_lookup,
            conda_packages,
            pypi_packages,
            pypi_environment_package_data: pypi_runtime_lookup
                .into_iter()
                .map(Into::into)
                .collect(),
        }),
    })
}

/// Parses a V7+ lock file directly to `CondaPackageData`.
fn parse_from_lock<P>(
    file_version: FileFormatVersion,
    raw: DeserializableLockFile<P>,
    base_dir: Option<&Path>,
) -> Result<LockFile, ParseCondaLockError> {
    // Split the packages into conda and pypi packages.
    let mut conda_packages = Vec::new();
    let mut pypi_packages = Vec::new();

    for package in raw.packages {
        match package {
            PackageData::Conda(p) => conda_packages.push(p),
            PackageData::Pypi(p) => {
                pypi_packages.push(convert_raw_pypi_package(file_version, p, base_dir)?);
            }
        }
    }

    // Determine the indices of the packages by url
    let mut conda_url_lookup: ahash::HashMap<UrlOrPath, Vec<_>> = ahash::HashMap::default();
    for (idx, conda_package) in conda_packages.iter().enumerate() {
        conda_url_lookup
            .entry(conda_package.location().clone())
            .or_default()
            .push(idx);
    }

    let pypi_url_lookup = pypi_packages
        .iter()
        .enumerate()
        .map(|(idx, p)| (&p.location, idx))
        .collect::<ahash::HashMap<_, _>>();
    let mut pypi_runtime_lookup = IndexSet::new();

    let environments = raw
        .environments
        .into_iter()
        .map(|(environment_name, env)| {
            Ok((
                environment_name.clone(),
                EnvironmentData {
                    channels: env.channels,
                    indexes: env.indexes,
                    options: env.options,
                    packages: env
                        .packages
                        .into_iter()
                        .map(|(platform, packages)| {
                            let packages = packages
                                .into_iter()
                                .map(|p| {
                                    Ok(match p {
                                        DeserializablePackageSelector::Conda {
                                            conda,
                                            name,
                                            version,
                                            build,
                                            subdir,
                                        } => {
                                            let all_packages = conda_url_lookup
                                                .get(&conda)
                                                .map_or(&[] as &[usize], Vec::as_slice);

                                            // V7+ uses exact field matching for disambiguation
                                            let package_index = all_packages
                                                .iter()
                                                .find(|&idx| {
                                                    let conda_package = &conda_packages[*idx];
                                                    name.as_ref().is_none_or(|name| {
                                                        name == &conda_package.record().name
                                                    }) && version.as_ref().is_none_or(|version| {
                                                        version == &conda_package.record().version
                                                    }) && build.as_ref().is_none_or(|build| {
                                                        build == &conda_package.record().build
                                                    }) && subdir.as_ref().is_none_or(|subdir| {
                                                        subdir == &conda_package.record().subdir
                                                    })
                                                })
                                                .copied();

                                            EnvironmentPackageData::Conda(
                                                package_index.ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        environment_name.clone(),
                                                        platform,
                                                        conda,
                                                    )
                                                })?,
                                            )
                                        }
                                        DeserializablePackageSelector::Pypi { pypi, runtime } => {
                                            EnvironmentPackageData::Pypi(
                                                *pypi_url_lookup.get(&pypi).ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        environment_name.clone(),
                                                        platform,
                                                        pypi.inner().clone(),
                                                    )
                                                })?,
                                                pypi_runtime_lookup.insert_full(runtime).0,
                                            )
                                        }
                                    })
                                })
                                .collect::<Result<_, ParseCondaLockError>>()?;

                            Ok((platform, packages))
                        })
                        .collect::<Result<_, ParseCondaLockError>>()?,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, ParseCondaLockError>>()?;

    let (environment_lookup, environments) = environments
        .into_iter()
        .enumerate()
        .map(|(idx, (name, env))| ((name, idx), env))
        .unzip();

    Ok(LockFile {
        inner: Arc::new(LockFileInner {
            version: file_version,
            environments,
            environment_lookup,
            conda_packages,
            pypi_packages,
            pypi_environment_package_data: pypi_runtime_lookup
                .into_iter()
                .map(Into::into)
                .collect(),
        }),
    })
}
