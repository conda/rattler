use std::{
    borrow::Cow, collections::BTreeMap, marker::PhantomData, path::Path, str::FromStr as _,
    sync::Arc,
};

use ahash::HashMapExt;
use indexmap::IndexSet;
use pep440_rs::VersionSpecifiers;
use rattler_conda_types::{PackageName, VersionWithSource};
use serde::{de::Error, Deserialize, Deserializer};
use serde_with::{serde_as, DeserializeAs};
use serde_yaml::Value;

use crate::{
    file_format_version::FileFormatVersion,
    parse::{
        models::{self, legacy::LegacyCondaPackageData, v6, v7},
        V5, V6, V7,
    },
    Channel, CondaBinaryData, CondaPackageData, CondaSourceData, EnvironmentData,
    EnvironmentPackageData, LockFile, LockFileInner, PackageHashes, ParseCondaLockError,
    PypiIndexes, PypiPackageData, SolveOptions, SourceIdentifier, UrlOrPath, Verbatim,
};

#[serde_as]
#[derive(Deserialize)]
#[serde(bound(deserialize = "P: DeserializeAs<'de, PackageData>"))]
struct DeserializableLockFile<P> {
    #[serde(default)]
    platforms: Vec<DeserializablePlatformData>,
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
    environments: BTreeMap<String, LegacyEnvironment>,
    #[serde_as(as = "Vec<P>")]
    packages: Vec<LegacyPackageData>,
    #[serde(skip)]
    _data: PhantomData<P>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
struct DeserializablePlatformData {
    name: String,
    #[serde(default)]
    subdir: Option<String>,
    #[serde(default)]
    virtual_packages: Vec<String>,
}

impl TryFrom<&DeserializablePlatformData> for crate::platform::PlatformData {
    type Error = crate::platform::ParsePlatformError;

    fn try_from(value: &DeserializablePlatformData) -> Result<Self, Self::Error> {
        let subdir = value.subdir.as_ref().map_or_else(
            || rattler_conda_types::Platform::from_str(&value.name),
            |s| rattler_conda_types::Platform::from_str(s),
        )?;

        Ok(Self {
            name: crate::platform::PlatformName::try_from(value.name.clone())?,
            subdir,
            virtual_packages: value.virtual_packages.clone(),
        })
    }
}

/// A pinned Pypi package
#[derive(Eq, PartialEq, Clone, Debug, Hash)]
pub struct PypiPackageDataRaw {
    /// The name of the package.
    pub name: pep508_rs::PackageName,

    /// The version of the package.
    pub version: Option<pep440_rs::Version>,

    /// The location of the package. This can be a URL or a path.
    pub location: Verbatim<UrlOrPath>,

    /// Hashes of the file pointed to by `url`.
    pub hash: Option<PackageHashes>,

    /// The index url used to retrieve this package.
    pub index_url: Option<url::Url>,

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
            .collect();

        Self {
            name: value.name.clone(),
            version: value.version.clone(),
            location: value.location.clone(),
            hash: value.hash.clone(),
            index_url: value.index_url.clone(),
            requires_dist,
            requires_python: value.requires_python.clone(),
        }
    }
}

/// Package data enum used during V7+ deserialization.
///
/// Source packages carry their `SourceIdentifier` separately so we can use it
/// directly for lookups without recomputing the hash.
#[allow(clippy::large_enum_variant)]
enum PackageData {
    /// Binary conda package.
    Conda(CondaBinaryData),
    /// Source conda package with its identifier for lookup.
    CondaSource(SourceIdentifier, CondaSourceData),
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

/// Environment struct for V7+ deserialization.
#[derive(Deserialize)]
struct DeserializableEnvironment {
    channels: Vec<Channel>,
    #[serde(flatten)]
    indexes: Option<PypiIndexes>,
    #[serde(default)]
    options: SolveOptions,
    packages: BTreeMap<String, Vec<DeserializablePackageSelector>>,
}

/// Environment struct for legacy (V4-V6) deserialization.
///
/// Uses `LegacyPackageSelector` which only supports `conda` and `pypi` keys,
/// not the `source` key introduced in V7.
#[derive(Deserialize)]
struct LegacyEnvironment {
    channels: Vec<Channel>,
    #[serde(flatten)]
    indexes: Option<PypiIndexes>,
    #[serde(default)]
    options: SolveOptions,
    packages: BTreeMap<rattler_conda_types::Platform, Vec<LegacyPackageSelector>>,
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
            Source {
                #[serde(rename = "source")]
                _source: String,
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
                "expected at least `conda`, `source`, or `pypi` field",
            ));
        };

        let deserializer = serde_value::ValueDeserializer::<D::Error>::new(value);
        Ok(match discriminant {
            Discriminant::Conda { .. } => PackageData::Conda(
                v7::CondaPackageDataModel::deserialize(deserializer)?
                    .try_into()
                    .map_err(D::Error::custom)?,
            ),
            Discriminant::Source { .. } => {
                let (identifier, source_data) =
                    v7::SourcePackageDataModel::deserialize(deserializer)?
                        .into_parts()
                        .map_err(D::Error::custom)?;
                PackageData::CondaSource(identifier, source_data)
            }
            Discriminant::Pypi { .. } => {
                PackageData::Pypi(v7::PypiPackageDataModel::deserialize(deserializer)?.into())
            }
        })
    }
}

/// Package selector for V7+ environments.
///
/// Supports `conda`, `source`, and `pypi` keys.
/// For V7+, binary conda packages are uniquely identified by their URL (which includes the
/// filename), and source packages use `SourceIdentifier` with an embedded hash.
#[derive(Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
enum DeserializablePackageSelector {
    /// Binary conda packages are uniquely identified by their URL.
    Conda {
        conda: UrlOrPath,
    },
    /// Source packages use `SourceIdentifier` format: `name[hash] @ location`.
    /// The hash uniquely identifies the package, so no additional disambiguation fields needed.
    Source {
        source: SourceIdentifier,
    },
    Pypi {
        pypi: Verbatim<UrlOrPath>,
    },
}

/// Package selector for legacy (V4-V6) environments.
///
/// Only supports `conda` and `pypi` keys. The `source` key was introduced in V7.
#[derive(Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
enum LegacyPackageSelector {
    Conda {
        conda: UrlOrPath,
        name: Option<PackageName>,
        version: Option<VersionWithSource>,
        build: Option<String>,
        subdir: Option<String>,
    },
    Pypi {
        pypi: Verbatim<UrlOrPath>,
    },
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
        index_url: raw_package.index_url,
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
    platform_name: &str,
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
        .filter(|&&idx| packages[idx].record().subdir.as_str() == platform_name)
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
    let platforms = create_legacy_platforms(&raw)?;

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

    let environments = raw
        .environments
        .into_iter()
        .map(|(env_name, env)| {
            Ok((
                env_name.clone(),
                EnvironmentData {
                    channels: env.channels,
                    indexes: env.indexes,
                    options: env.options,
                    packages: env
                        .packages
                        .into_iter()
                        .map(|(platform, packages)| {
                            let platform_name = platform.to_string();
                            let Some((platform_index, _)) = platforms
                                .iter()
                                .enumerate()
                                .find(|(_, p)| p.name.as_str() == platform_name.as_str())
                            else {
                                return Err(ParseCondaLockError::UnknownPlatform {
                                    environment: env_name.clone(),
                                    platform: platform_name,
                                });
                            };

                            let packages = packages
                                .into_iter()
                                .map(|p| {
                                    Ok(match p {
                                        LegacyPackageSelector::Conda {
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
                                                platform.as_str(),
                                                name.as_ref(),
                                                version.as_ref(),
                                                build.as_deref(),
                                                subdir.as_deref(),
                                                candidate_indices,
                                                &mut legacy_conda_packages,
                                            );

                                            EnvironmentPackageData::Conda(
                                                package_index.ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage {
                                                        environment: env_name.clone(),
                                                        platform: platform_name.clone(),
                                                        location: conda.to_string(),
                                                    }
                                                })?,
                                            )
                                        }
                                        LegacyPackageSelector::Pypi { pypi } => {
                                            EnvironmentPackageData::Pypi(
                                                *pypi_url_lookup.get(&pypi).ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage {
                                                        environment: env_name.clone(),
                                                        platform: platform_name.clone(),
                                                        location: pypi.inner().to_string(),
                                                    }
                                                })?,
                                            )
                                        }
                                    })
                                })
                                .collect::<Result<_, ParseCondaLockError>>()?;

                            Ok((platform_index, packages))
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
            platforms,
            environments,
            environment_lookup,
            conda_packages,
            pypi_packages,
        }),
    })
}

/// Create a new `Vec<Platform>` from a legacy lock file.
///
/// Iterate over the environments and take the `rattler_conda_types::Platform`
/// listed there and turn those into `Platform`.
fn create_legacy_platforms<P>(
    raw: &DeserializableLockFileLegacy<P>,
) -> Result<Vec<crate::platform::PlatformData>, ParseCondaLockError> {
    let mut unique_platforms = ahash::HashSet::default();
    raw.environments
        .iter()
        .flat_map(|(env_name, env)| {
            env.packages
                .keys()
                .map(move |platform| (env_name, platform))
        })
        .filter(move |(_, platform)| unique_platforms.insert(*platform))
        .map(|(_, subdir)| -> Result<_, ParseCondaLockError> {
            let name = crate::platform::PlatformName::try_from(subdir.as_str())?;

            Ok(crate::platform::PlatformData {
                name,
                subdir: *subdir,
                virtual_packages: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, _>>()
}

/// Extract a `Vec<Platform>` from the lock file.
fn read_platforms<P>(
    raw: &DeserializableLockFile<P>,
) -> Result<Vec<crate::platform::PlatformData>, ParseCondaLockError> {
    let mut unique_platforms = ahash::HashSet::default();
    raw.platforms
        .iter()
        .map(crate::platform::PlatformData::try_from)
        .map(move |platform| match platform {
            Ok(platform) => {
                if unique_platforms.insert(platform.name.clone()) {
                    Ok(platform)
                } else {
                    Err(ParseCondaLockError::DuplicatePlatformName(
                        platform.name.to_string(),
                    ))
                }
            }
            Err(e) => Err(e.into()),
        })
        .collect::<Result<Vec<_>, _>>()
}

/// Parses a V7+ lock file directly to `CondaPackageData`.
fn parse_from_lock<P>(
    file_version: FileFormatVersion,
    raw: DeserializableLockFile<P>,
    base_dir: Option<&Path>,
) -> Result<LockFile, ParseCondaLockError> {
    let platforms = read_platforms(&raw)?;

    // Split the packages into conda and pypi packages, building lookups as we go.
    // Binary packages are uniquely identified by URL.
    // Source packages are uniquely identified by SourceIdentifier (name[hash] @ location).
    let num_packages = raw.packages.len();
    let mut conda_packages: Vec<CondaPackageData> = Vec::with_capacity(num_packages);
    let mut pypi_packages = Vec::with_capacity(num_packages);
    let mut binary_url_lookup: ahash::HashMap<UrlOrPath, usize> =
        ahash::HashMap::with_capacity(num_packages);
    let mut source_identifier_lookup: ahash::HashMap<SourceIdentifier, usize> =
        ahash::HashMap::with_capacity(num_packages);

    for package in raw.packages {
        match package {
            PackageData::Conda(binary) => {
                let idx = conda_packages.len();
                binary_url_lookup.insert(binary.location.clone(), idx);
                conda_packages.push(CondaPackageData::Binary(binary));
            }
            PackageData::CondaSource(identifier, source_data) => {
                let idx = conda_packages.len();
                source_identifier_lookup.insert(identifier, idx);
                conda_packages.push(CondaPackageData::Source(source_data));
            }
            PackageData::Pypi(p) => {
                pypi_packages.push(convert_raw_pypi_package(file_version, p, base_dir)?);
            }
        }
    }

    let pypi_url_lookup: ahash::HashMap<_, _> = pypi_packages
        .iter()
        .enumerate()
        .map(|(idx, p)| (&p.location, idx))
        .collect();

    // Parse environments
    let num_environments = raw.environments.len();
    let mut environments = Vec::with_capacity(num_environments);
    let mut environment_lookup: ahash::HashMap<String, usize> =
        ahash::HashMap::with_capacity(num_environments);

    for (env_name, env) in raw.environments {
        let mut env_packages: ahash::HashMap<usize, IndexSet<EnvironmentPackageData>> =
            ahash::HashMap::with_capacity(env.packages.len());

        for (platform_name, selectors) in env.packages {
            let Some((platform_index, platform_data)) = platforms
                .iter()
                .enumerate()
                .find(|(_, p)| p.name.as_str() == platform_name)
            else {
                return Err(ParseCondaLockError::UnknownPlatform {
                    environment: env_name.clone(),
                    platform: platform_name,
                });
            };

            let mut resolved = IndexSet::with_capacity(selectors.len());

            for selector in selectors {
                let package_data = resolve_package_selector(
                    selector,
                    &env_name,
                    platform_data.subdir,
                    &binary_url_lookup,
                    &source_identifier_lookup,
                    &pypi_url_lookup,
                )?;
                resolved.insert(package_data);
            }

            env_packages.insert(platform_index, resolved);
        }

        environment_lookup.insert(env_name, environments.len());
        environments.push(EnvironmentData {
            channels: env.channels,
            indexes: env.indexes,
            options: env.options,
            packages: env_packages,
        });
    }

    Ok(LockFile {
        inner: Arc::new(LockFileInner {
            version: file_version,
            platforms,
            environments,
            environment_lookup,
            conda_packages,
            pypi_packages,
        }),
    })
}

/// Resolves a package selector to an `EnvironmentPackageData`.
fn resolve_package_selector(
    selector: DeserializablePackageSelector,
    env_name: &str,
    platform: rattler_conda_types::Platform,
    binary_url_lookup: &ahash::HashMap<UrlOrPath, usize>,
    source_identifier_lookup: &ahash::HashMap<SourceIdentifier, usize>,
    pypi_url_lookup: &ahash::HashMap<&Verbatim<UrlOrPath>, usize>,
) -> Result<EnvironmentPackageData, ParseCondaLockError> {
    match selector {
        DeserializablePackageSelector::Conda { conda } => {
            let idx = binary_url_lookup.get(&conda).ok_or_else(|| {
                ParseCondaLockError::MissingPackage {
                    environment: env_name.to_owned(),
                    platform: platform.to_string(),
                    location: conda.to_string(),
                }
            })?;
            Ok(EnvironmentPackageData::Conda(*idx))
        }
        DeserializablePackageSelector::Source { source } => {
            let idx = source_identifier_lookup.get(&source).ok_or_else(|| {
                ParseCondaLockError::MissingPackage {
                    environment: env_name.to_owned(),
                    platform: platform.to_string(),
                    location: source.to_string(),
                }
            })?;
            Ok(EnvironmentPackageData::Conda(*idx))
        }
        DeserializablePackageSelector::Pypi { pypi } => {
            let idx =
                pypi_url_lookup
                    .get(&pypi)
                    .ok_or_else(|| ParseCondaLockError::MissingPackage {
                        environment: env_name.to_owned(),
                        platform: platform.to_string(),
                        location: pypi.inner().to_string(),
                    })?;
            Ok(EnvironmentPackageData::Pypi(*idx))
        }
    }
}
