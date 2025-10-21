use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    sync::Arc,
};

use fxhash::FxHashMap;
use indexmap::IndexSet;
use pep508_rs::ExtraName;
use rattler_conda_types::{PackageName, Platform, VersionWithSource};
use serde::{de::Error, Deserialize, Deserializer};
use serde_with::{serde_as, DeserializeAs};
use serde_yaml::Value;

use crate::{
    file_format_version::FileFormatVersion,
    parse::{models, models::v6, V5, V6, V7},
    Channel, CondaPackageData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    ParseCondaLockError, PypiIndexes, PypiPackageData, PypiPackageEnvironmentData, SolveOptions,
    UrlOrPath,
};

/// Schema-agnostic lock-file representation used during deserialization.
///
/// `P` controls how individual package entries are mapped (it picks the
/// version-specific model, e.g. v5 vs v6), while `S` defines the selector type
/// that environments emit for those packages in this file format.
#[serde_as]
#[derive(Deserialize)]
#[serde(bound(deserialize = "P: DeserializeAs<'de, PackageData>, S: Deserialize<'de>"))]
struct DeserializableLockFile<P, S> {
    environments: BTreeMap<String, DeserializableEnvironment<S>>,
    #[serde_as(as = "Vec<P>")]
    packages: Vec<PackageData>,
    #[serde(skip)]
    _marker: PhantomData<(P, S)>,
}

#[allow(clippy::large_enum_variant)]
enum PackageData {
    Conda(CondaPackageData),
    Pypi(PypiPackageData),
}

/// Environment payload as stored on disk for a specific format version.
///
/// `S` is the selector representation for that version (used later to resolve
/// actual package indices inside `parse_from_lock`).
#[derive(Deserialize)]
#[serde(bound(deserialize = "S: Deserialize<'de>"))]
struct DeserializableEnvironment<S> {
    channels: Vec<Channel>,
    #[serde(flatten)]
    indexes: Option<PypiIndexes>,
    #[serde(default)]
    options: SolveOptions,
    packages: BTreeMap<Platform, Vec<S>>,
}

impl<'de> DeserializeAs<'de, PackageData> for V5 {
    fn deserialize_as<D>(deserializer: D) -> Result<PackageData, D::Error>
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
            Inner::Conda(c) => PackageData::Conda(c.into()),
            Inner::Pypi(p) => PackageData::Pypi(p.into()),
        })
    }
}

impl<'de> DeserializeAs<'de, PackageData> for V6 {
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
                v6::CondaPackageDataModel::deserialize(deserializer)?
                    .try_into()
                    .map_err(D::Error::custom)?,
            ),
            Discriminant::Pypi { .. } => {
                PackageData::Pypi(v6::PypiPackageDataModel::deserialize(deserializer)?.into())
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
                models::v7::CondaPackageDataModel::deserialize(deserializer)?
                    .try_into()
                    .map_err(D::Error::custom)?,
            ),
            Discriminant::Pypi { .. } => PackageData::Pypi(
                models::v7::PypiPackageDataModel::deserialize(deserializer)?.into(),
            ),
        })
    }
}

#[derive(Deserialize)]
struct CondaSelectorV6 {
    #[serde(rename = "conda")]
    conda: UrlOrPath,
    name: Option<PackageName>,
    version: Option<VersionWithSource>,
    build: Option<String>,
    subdir: Option<String>,
}

#[derive(Deserialize)]
struct PypiSelector {
    #[serde(rename = "pypi")]
    pypi: UrlOrPath,
    #[serde(flatten)]
    runtime: DeserializablePypiPackageEnvironmentData,
}

#[derive(Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
enum DeserializablePackageSelectorV6 {
    Conda(CondaSelectorV6),
    Pypi(PypiSelector),
}

type DeserializablePackageSelectorV5 = DeserializablePackageSelectorV6;

// V7 selectors - initially identical to V6, will support variants later
#[derive(Deserialize)]
struct CondaSelectorV7 {
    #[serde(rename = "conda")]
    conda: UrlOrPath,
    name: Option<PackageName>,
    version: Option<VersionWithSource>,
    build: Option<String>,
    subdir: Option<String>,
    // TODO: Add variants field in later steps for V7-specific disambiguation
}

#[derive(Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
enum DeserializablePackageSelectorV7 {
    Conda(CondaSelectorV7),
    Pypi(PypiSelector),
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

/// Shared context used while resolving selectors into concrete package indices.
struct ResolveCtx<'a> {
    file_version: FileFormatVersion,
    environment_name: &'a str,
    platform: Platform,
    conda_packages: &'a mut Vec<CondaPackageData>,
    conda_url_lookup: &'a FxHashMap<UrlOrPath, Vec<usize>>,
    pypi_url_lookup: &'a FxHashMap<UrlOrPath, usize>,
    pypi_runtime_lookup: &'a mut IndexSet<DeserializablePypiPackageEnvironmentData>,
}

/// Trait implemented by version-specific selector enums so they can resolve
/// themselves into canonical `EnvironmentPackageData` entries.
trait PackageSelector {
    fn resolve(
        self,
        ctx: &mut ResolveCtx<'_>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError>;
}

impl PackageSelector for DeserializablePackageSelectorV6 {
    fn resolve(
        self,
        ctx: &mut ResolveCtx<'_>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError> {
        match self {
            DeserializablePackageSelectorV6::Conda(selector) => {
                resolve_conda_selector(selector, ctx)
            }
            DeserializablePackageSelectorV6::Pypi(selector) => resolve_pypi_selector(selector, ctx),
        }
    }
}

impl PackageSelector for DeserializablePackageSelectorV7 {
    fn resolve(
        self,
        ctx: &mut ResolveCtx<'_>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError> {
        match self {
            DeserializablePackageSelectorV7::Conda(selector) => {
                // Convert V7 selector to V6 selector format
                let v6_selector = CondaSelectorV6 {
                    conda: selector.conda,
                    name: selector.name,
                    version: selector.version,
                    build: selector.build,
                    subdir: selector.subdir,
                };
                resolve_conda_selector(v6_selector, ctx)
            }
            DeserializablePackageSelectorV7::Pypi(selector) => resolve_pypi_selector(selector, ctx),
        }
    }
}

/// Resolve a v6+ conda selector into the correct package index, using either
/// legacy merge behaviour or strict field matching depending on the file version.
fn resolve_conda_selector(
    selector: CondaSelectorV6,
    ctx: &mut ResolveCtx<'_>,
) -> Result<EnvironmentPackageData, ParseCondaLockError> {
    let CondaSelectorV6 {
        conda,
        name,
        version,
        build,
        subdir,
    } = selector;

    let candidates = ctx
        .conda_url_lookup
        .get(&conda)
        .map_or(&[] as &[usize], Vec::as_slice);

    let package_index = if ctx.file_version < FileFormatVersion::V6 {
        resolve_conda_selector_pre_v6(ctx.conda_packages, candidates, ctx.platform)
    } else {
        resolve_conda_selector_v6(
            ctx.conda_packages.as_slice(),
            candidates,
            name.as_ref(),
            version.as_ref(),
            build.as_deref(),
            subdir.as_deref(),
        )
    };

    let package_index = package_index.ok_or_else(|| {
        ParseCondaLockError::MissingPackage(
            ctx.environment_name.to_string(),
            ctx.platform,
            conda.clone(),
        )
    })?;

    Ok(EnvironmentPackageData::Conda(package_index))
}

/// Legacy (pre-v6) selector resolution that merges duplicate records when only
/// location information is available.
fn resolve_conda_selector_pre_v6(
    conda_packages: &mut Vec<CondaPackageData>,
    candidates: &[usize],
    platform: Platform,
) -> Option<usize> {
    let mut indices: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&idx| conda_packages[idx].record().subdir.as_str() == platform.as_str())
        .collect();

    if indices.is_empty() {
        indices.extend_from_slice(candidates);
    }

    let mut iter = indices.into_iter();
    let first_package_idx = iter.next()?;

    let merged_package = iter.fold(
        Cow::Borrowed(&conda_packages[first_package_idx]),
        |acc, next_package_idx| {
            if let Cow::Owned(merged) = acc.merge(&conda_packages[next_package_idx]) {
                Cow::Owned(merged)
            } else {
                acc
            }
        },
    );

    Some(match merged_package {
        Cow::Borrowed(_) => first_package_idx,
        Cow::Owned(package) => {
            conda_packages.push(package);
            conda_packages.len() - 1
        }
    })
}

/// Modern selector resolution relying on explicit metadata (name/version/build/subdir).
fn resolve_conda_selector_v6(
    conda_packages: &[CondaPackageData],
    candidates: &[usize],
    name: Option<&PackageName>,
    version: Option<&VersionWithSource>,
    build: Option<&str>,
    subdir: Option<&str>,
) -> Option<usize> {
    candidates
        .iter()
        .find(|&&idx| {
            let conda_package = &conda_packages[idx];
            let record = conda_package.record();
            let record_name = &record.name;
            let record_version = &record.version;
            let record_build = record.build.as_str();
            let record_subdir = record.subdir.as_str();
            name.as_ref()
                .is_none_or(|expected| *expected == record_name)
                && version
                    .as_ref()
                    .is_none_or(|expected| *expected == record_version)
                && build.is_none_or(|expected| expected == record_build)
                && subdir.is_none_or(|expected| expected == record_subdir)
        })
        .copied()
}

/// Resolve a PyPI selector into the shared package index while deduplicating
/// environment runtime configurations.
fn resolve_pypi_selector(
    selector: PypiSelector,
    ctx: &mut ResolveCtx<'_>,
) -> Result<EnvironmentPackageData, ParseCondaLockError> {
    let index = *ctx.pypi_url_lookup.get(&selector.pypi).ok_or_else(|| {
        ParseCondaLockError::MissingPackage(
            ctx.environment_name.to_string(),
            ctx.platform,
            selector.pypi.clone(),
        )
    })?;

    let runtime_idx = ctx.pypi_runtime_lookup.insert_full(selector.runtime).0;

    Ok(EnvironmentPackageData::Pypi(index, runtime_idx))
}

/// Parses a [`LockFile`] from a [`serde_yaml::Value`].
pub fn parse_from_document_v5(
    document: Value,
    version: FileFormatVersion,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFile<V5, DeserializablePackageSelectorV5> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock(version, raw)
}

pub fn parse_from_document_v6(
    document: Value,
    version: FileFormatVersion,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFile<V6, DeserializablePackageSelectorV6> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock(version, raw)
}

pub fn parse_from_document_v7(
    document: Value,
    version: FileFormatVersion,
) -> Result<LockFile, ParseCondaLockError> {
    // V7 uses its own models and selectors (currently identical to V6 but can evolve independently)
    let raw: DeserializableLockFile<V7, DeserializablePackageSelectorV7> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock(version, raw)
}

/// Convert the intermediate lock-file representation into the canonical in-memory model.
fn parse_from_lock<P, S>(
    file_version: FileFormatVersion,
    raw: DeserializableLockFile<P, S>,
) -> Result<LockFile, ParseCondaLockError>
where
    S: PackageSelector,
{
    let DeserializableLockFile {
        environments: raw_environments,
        packages: raw_packages,
        ..
    } = raw;

    // Split the packages into conda and pypi packages.
    let mut conda_packages = Vec::new();
    let mut pypi_packages = Vec::new();
    for package in raw_packages {
        match package {
            PackageData::Conda(p) => conda_packages.push(p),
            PackageData::Pypi(p) => pypi_packages.push(p),
        }
    }

    // Determine the indices of the packages by url
    let mut conda_url_lookup: FxHashMap<UrlOrPath, Vec<_>> = FxHashMap::default();
    for (idx, conda_package) in conda_packages.iter().enumerate() {
        conda_url_lookup
            .entry(conda_package.location().clone())
            .or_default()
            .push(idx);
    }

    let pypi_url_lookup = pypi_packages
        .iter()
        .enumerate()
        .map(|(idx, package)| (package.location.clone(), idx))
        .collect::<FxHashMap<_, _>>();
    let mut pypi_runtime_lookup = IndexSet::new();

    let environments = raw_environments
        .into_iter()
        .map(|(environment_name, env)| {
            let DeserializableEnvironment {
                channels,
                indexes,
                options,
                packages,
            } = env;

            let mut packages_by_platform = FxHashMap::default();
            for (platform, selectors) in packages {
                let mut ctx = ResolveCtx {
                    file_version,
                    environment_name: &environment_name,
                    platform,
                    conda_packages: &mut conda_packages,
                    conda_url_lookup: &conda_url_lookup,
                    pypi_url_lookup: &pypi_url_lookup,
                    pypi_runtime_lookup: &mut pypi_runtime_lookup,
                };

                let mut platform_packages = IndexSet::new();
                for selector in selectors {
                    let package = selector.resolve(&mut ctx)?;
                    platform_packages.insert(package);
                }

                packages_by_platform.insert(platform, platform_packages);
            }

            Ok((
                environment_name.clone(),
                EnvironmentData {
                    channels,
                    indexes,
                    options,
                    packages: packages_by_platform,
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
