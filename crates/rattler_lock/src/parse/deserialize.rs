use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    sync::Arc,
};

use indexmap::IndexSet;
use pep508_rs::ExtraName;
use rattler_conda_types::Platform;
use serde::Deserialize;
use serde_yaml::Value;

use crate::{
    file_format_version::FileFormatVersion,
    parse::{V5, V6, V7},
    Channel, CondaPackageData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    ParseCondaLockError, PypiIndexes, PypiPackageData, PypiPackageEnvironmentData, SolveOptions,
    UrlOrPath,
};

/// Helper trait for types that have a location
pub(crate) trait HasLocation {
    fn location(&self) -> &UrlOrPath;
}

/// Trait defining version-specific types for lock file parsing.
///
/// Each lock file version (V5, V6, V7) implements this trait to specify:
/// - The package data representation (models vs converted types)
/// - The selector type used in environments
/// - How to resolve selectors to package indices
pub(crate) trait LockFileVersion {
    /// The package data type for this version (e.g., `PackageDataV5`, `PackageDataV6`)
    type PackageData: for<'de> Deserialize<'de>;

    /// The selector type used in environment package lists
    type Selector: for<'de> Deserialize<'de> + PackageSelector<Self::CondaPackage>;

    /// The conda package representation during resolution (model or final type)
    /// Must be convertible to `CondaPackageData` after resolution completes
    type CondaPackage: TryInto<CondaPackageData> + HasLocation;

    /// Extract conda and pypi packages from the package data list (keeping models for resolution)
    fn extract_packages(
        packages: Vec<Self::PackageData>,
    ) -> Result<(Vec<Self::CondaPackage>, Vec<PypiPackageData>), ParseCondaLockError>;
}

/// Schema-agnostic lock-file representation used during deserialization.
///
/// `V` is the lock file version marker (V5, V6, V7) which specifies the types via `LockFileVersion` trait
#[derive(Deserialize)]
#[serde(bound(deserialize = "V: LockFileVersion"))]
struct DeserializableLockFile<V>
where
    V: LockFileVersion,
{
    environments: BTreeMap<String, DeserializableEnvironment<V::Selector>>,
    packages: Vec<V::PackageData>,
    #[serde(skip)]
    _marker: PhantomData<V>,
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

#[derive(Deserialize)]
pub(crate) struct PypiSelector {
    #[serde(rename = "pypi")]
    pub(crate) pypi: UrlOrPath,
    #[serde(flatten)]
    pub(crate) runtime: DeserializablePypiPackageEnvironmentData,
}

#[derive(Hash, Deserialize, Eq, PartialEq)]
pub(crate) struct DeserializablePypiPackageEnvironmentData {
    #[serde(default)]
    pub(crate) extras: BTreeSet<ExtraName>,
}

impl From<DeserializablePypiPackageEnvironmentData> for PypiPackageEnvironmentData {
    fn from(config: DeserializablePypiPackageEnvironmentData) -> Self {
        Self {
            extras: config.extras.into_iter().collect(),
        }
    }
}

/// Shared context used while resolving selectors into concrete package indices.
pub(crate) struct ResolveCtx<'a, CP> {
    pub(crate) environment_name: &'a str,
    pub(crate) platform: Platform,
    pub(crate) conda_packages: &'a mut Vec<CP>,
    pub(crate) conda_url_lookup: &'a ahash::HashMap<UrlOrPath, Vec<usize>>,
    pub(crate) pypi_url_lookup: &'a ahash::HashMap<UrlOrPath, usize>,
    pub(crate) pypi_runtime_lookup: &'a mut IndexSet<DeserializablePypiPackageEnvironmentData>,
}

/// Trait implemented by version-specific selector enums so they can resolve
/// themselves into canonical `EnvironmentPackageData` entries.
pub(crate) trait PackageSelector<CP> {
    fn resolve(
        self,
        ctx: &mut ResolveCtx<'_, CP>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError>;
}

/// Resolve a `PyPI` selector into the shared package index while deduplicating
/// environment runtime configurations.
pub(crate) fn resolve_pypi_selector<CP>(
    selector: PypiSelector,
    ctx: &mut ResolveCtx<'_, CP>,
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
    let raw: DeserializableLockFile<V5> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock::<V5>(version, raw)
}

pub fn parse_from_document_v6(
    document: Value,
    version: FileFormatVersion,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFile<V6> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock::<V6>(version, raw)
}

pub fn parse_from_document_v7(
    document: Value,
    version: FileFormatVersion,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFile<V7> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock::<V7>(version, raw)
}

/// Resolve all packages for a single environment across all platforms.
///
/// This function resolves version-specific package selectors into canonical
/// `EnvironmentPackageData` entries, handling disambiguation and PyPI runtime
/// configuration lookup.
#[allow(clippy::doc_markdown)]
fn process_environment_packages<V>(
    environment_name: String,
    packages: BTreeMap<Platform, Vec<V::Selector>>,
    conda_packages: &mut Vec<V::CondaPackage>,
    conda_url_lookup: &ahash::HashMap<UrlOrPath, Vec<usize>>,
    pypi_url_lookup: &ahash::HashMap<UrlOrPath, usize>,
    pypi_runtime_lookup: &mut IndexSet<DeserializablePypiPackageEnvironmentData>,
) -> Result<
    (
        String,
        ahash::HashMap<Platform, IndexSet<EnvironmentPackageData>>,
    ),
    ParseCondaLockError,
>
where
    V: LockFileVersion,
    <V::CondaPackage as TryInto<CondaPackageData>>::Error: Into<ParseCondaLockError>,
{
    let mut packages_by_platform = ahash::HashMap::default();

    for (platform, selectors) in packages {
        let mut ctx = ResolveCtx {
            environment_name: &environment_name,
            platform,
            conda_packages,
            conda_url_lookup,
            pypi_url_lookup,
            pypi_runtime_lookup,
        };

        let platform_packages = selectors
            .into_iter()
            .map(|selector| selector.resolve(&mut ctx))
            .collect::<Result<IndexSet<_>, _>>()?;

        packages_by_platform.insert(platform, platform_packages);
    }

    Ok((environment_name, packages_by_platform))
}

/// Convert the lock-file representation into the canonical in-memory model.
fn parse_from_lock<V>(
    file_version: FileFormatVersion,
    raw: DeserializableLockFile<V>,
) -> Result<LockFile, ParseCondaLockError>
where
    V: LockFileVersion,
    <V::CondaPackage as TryInto<CondaPackageData>>::Error: Into<ParseCondaLockError>,
{
    let DeserializableLockFile {
        environments: raw_environments,
        packages: raw_packages,
        ..
    } = raw;

    // Extract and convert packages to final types
    let (mut conda_packages, pypi_packages) = V::extract_packages(raw_packages)?;

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
        .map(|(idx, p)| (p.location.clone(), idx))
        .collect::<ahash::HashMap<_, _>>();
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

            let (env_name, packages_by_platform) = process_environment_packages::<V>(
                environment_name,
                packages,
                &mut conda_packages,
                &conda_url_lookup,
                &pypi_url_lookup,
                &mut pypi_runtime_lookup,
            )?;

            Ok((
                env_name,
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

    // Convert models to final CondaPackageData after all selector resolution is complete
    let final_conda_packages: Vec<CondaPackageData> = conda_packages
        .into_iter()
        .map(|model| model.try_into().map_err(Into::into))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(LockFile {
        inner: Arc::new(LockFileInner {
            version: file_version,
            environments,
            environment_lookup,
            conda_packages: final_conda_packages,
            pypi_packages,
            pypi_environment_package_data: pypi_runtime_lookup
                .into_iter()
                .map(Into::into)
                .collect(),
        }),
    })
}
