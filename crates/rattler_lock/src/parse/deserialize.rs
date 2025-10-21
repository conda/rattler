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
use serde::{Deserialize, Deserializer};
use serde_yaml::Value;

use crate::{
    file_format_version::FileFormatVersion,
    parse::{models, models::v6, V5, V6, V7},
    utils::derived_fields::LocationDerivedFields,
    Channel, CondaPackageData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    ParseCondaLockError, PypiIndexes, PypiPackageData, PypiPackageEnvironmentData, SolveOptions,
    UrlOrPath,
};

/// Helper trait for types that have a location
trait HasLocation {
    fn location(&self) -> &UrlOrPath;
}

impl HasLocation for CondaPackageData {
    fn location(&self) -> &UrlOrPath {
        CondaPackageData::location(self)
    }
}

impl HasLocation for v6::CondaPackageDataModel<'static> {
    fn location(&self) -> &UrlOrPath {
        &self.location
    }
}

impl HasLocation for models::v7::CondaPackageDataModel<'static> {
    fn location(&self) -> &UrlOrPath {
        &self.location
    }
}

/// Trait defining version-specific types for lock file parsing.
///
/// Each lock file version (V5, V6, V7) implements this trait to specify:
/// - The package data representation (models vs converted types)
/// - The selector type used in environments
/// - How to resolve selectors to package indices
trait LockFileVersion {
    /// The package data type for this version (e.g., PackageDataV5, PackageDataV6)
    type PackageData: for<'de> Deserialize<'de>;

    /// The selector type used in environment package lists
    type Selector: for<'de> Deserialize<'de>;

    /// The conda package representation during resolution (model or final type)
    /// Must be convertible to CondaPackageData after resolution completes
    type CondaPackage: TryInto<CondaPackageData> + HasLocation;

    /// Extract conda and pypi packages from the package data list (keeping models for resolution)
    fn extract_packages(
        packages: Vec<Self::PackageData>,
    ) -> Result<(Vec<Self::CondaPackage>, Vec<PypiPackageData>), ParseCondaLockError>;

    /// Resolve a selector to a package index
    fn resolve_selector(
        selector: Self::Selector,
        ctx: &mut ResolveCtx<'_, Self::CondaPackage>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError>;
}

/// Schema-agnostic lock-file representation used during deserialization.
///
/// `V` is the lock file version marker (V5, V6, V7) which specifies the types via LockFileVersion trait
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

/// V5-specific package data (stores final converted types)
#[allow(clippy::large_enum_variant)]
enum PackageDataV5 {
    Conda(CondaPackageData),
    Pypi(PypiPackageData),
}

/// V6-specific package data (stores models before conversion)
#[derive(Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
enum PackageDataV6<'a> {
    Conda(v6::CondaPackageDataModel<'a>),
    Pypi(v6::PypiPackageDataModel<'a>),
}

/// V7-specific package data (stores models before conversion)
#[derive(Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
enum PackageDataV7<'a> {
    Conda(models::v7::CondaPackageDataModel<'a>),
    Pypi(models::v7::PypiPackageDataModel<'a>),
}

// Implement LockFileVersion for V5
impl LockFileVersion for V5 {
    type PackageData = PackageDataV5;
    type Selector = DeserializablePackageSelectorV5;
    type CondaPackage = CondaPackageData;

    fn extract_packages(
        packages: Vec<Self::PackageData>,
    ) -> Result<(Vec<Self::CondaPackage>, Vec<PypiPackageData>), ParseCondaLockError> {
        let mut conda_packages = Vec::new();
        let mut pypi_packages = Vec::new();
        for package in packages {
            match package {
                PackageDataV5::Conda(p) => conda_packages.push(p),
                PackageDataV5::Pypi(p) => pypi_packages.push(p),
            }
        }
        Ok((conda_packages, pypi_packages))
    }

    fn resolve_selector(
        selector: Self::Selector,
        ctx: &mut ResolveCtx<'_, Self::CondaPackage>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError> {
        selector.resolve(ctx)
    }
}

// Implement LockFileVersion for V6
impl LockFileVersion for V6 {
    type PackageData = PackageDataV6<'static>;
    type Selector = DeserializablePackageSelectorV6;
    type CondaPackage = v6::CondaPackageDataModel<'static>;

    fn extract_packages(
        packages: Vec<Self::PackageData>,
    ) -> Result<(Vec<Self::CondaPackage>, Vec<PypiPackageData>), ParseCondaLockError> {
        let mut conda_packages = Vec::new();
        let mut pypi_packages = Vec::new();
        for package in packages {
            match package {
                PackageDataV6::Conda(model) => conda_packages.push(model),
                PackageDataV6::Pypi(model) => pypi_packages.push(model.into()),
            }
        }
        Ok((conda_packages, pypi_packages))
    }

    fn resolve_selector(
        selector: Self::Selector,
        ctx: &mut ResolveCtx<'_, Self::CondaPackage>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError> {
        selector.resolve(ctx)
    }
}

// Implement LockFileVersion for V7
impl LockFileVersion for V7 {
    type PackageData = PackageDataV7<'static>;
    type Selector = DeserializablePackageSelectorV7;
    type CondaPackage = models::v7::CondaPackageDataModel<'static>;

    fn extract_packages(
        packages: Vec<Self::PackageData>,
    ) -> Result<(Vec<Self::CondaPackage>, Vec<PypiPackageData>), ParseCondaLockError> {
        let mut conda_packages = Vec::new();
        let mut pypi_packages = Vec::new();
        for package in packages {
            match package {
                PackageDataV7::Conda(model) => conda_packages.push(model),
                PackageDataV7::Pypi(model) => pypi_packages.push(model.into()),
            }
        }
        Ok((conda_packages, pypi_packages))
    }

    fn resolve_selector(
        selector: Self::Selector,
        ctx: &mut ResolveCtx<'_, Self::CondaPackage>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError> {
        selector.resolve(ctx)
    }
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

// V5 uses tagged format and converts immediately
impl<'de> Deserialize<'de> for PackageDataV5 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
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
            Inner::Conda(c) => PackageDataV5::Conda(c.into()),
            Inner::Pypi(p) => PackageDataV5::Pypi(p.into()),
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

// V7 selectors - simplified, use variants for source package disambiguation
// Binary packages are unique by location, so no version/build/subdir needed
#[derive(Deserialize)]
struct CondaSelectorV7 {
    #[serde(rename = "conda")]
    conda: UrlOrPath,
    name: Option<PackageName>,
    // For source packages: variants-based disambiguation
    #[serde(default)]
    variants: BTreeMap<String, crate::VariantValue>,
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
struct ResolveCtx<'a, CP> {
    environment_name: &'a str,
    platform: Platform,
    conda_packages: &'a mut Vec<CP>,
    conda_url_lookup: &'a FxHashMap<UrlOrPath, Vec<usize>>,
    pypi_url_lookup: &'a FxHashMap<UrlOrPath, usize>,
    pypi_runtime_lookup: &'a mut IndexSet<DeserializablePypiPackageEnvironmentData>,
}

/// Trait implemented by version-specific selector enums so they can resolve
/// themselves into canonical `EnvironmentPackageData` entries.
trait PackageSelector<CP> {
    fn resolve(
        self,
        ctx: &mut ResolveCtx<'_, CP>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError>;
}

// V5 uses CondaPackageData directly
impl PackageSelector<CondaPackageData> for DeserializablePackageSelectorV5 {
    fn resolve(
        self,
        ctx: &mut ResolveCtx<'_, CondaPackageData>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError> {
        match self {
            DeserializablePackageSelectorV5::Conda(selector) => {
                resolve_conda_selector_v5(selector, ctx)
            }
            DeserializablePackageSelectorV5::Pypi(selector) => resolve_pypi_selector(selector, ctx),
        }
    }
}

impl PackageSelector<v6::CondaPackageDataModel<'static>> for DeserializablePackageSelectorV6 {
    fn resolve(
        self,
        ctx: &mut ResolveCtx<'_, v6::CondaPackageDataModel<'static>>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError> {
        match self {
            DeserializablePackageSelectorV6::Conda(selector) => {
                resolve_conda_selector_v6_models(selector, ctx)
            }
            DeserializablePackageSelectorV6::Pypi(selector) => resolve_pypi_selector(selector, ctx),
        }
    }
}

impl PackageSelector<models::v7::CondaPackageDataModel<'static>>
    for DeserializablePackageSelectorV7
{
    fn resolve(
        self,
        ctx: &mut ResolveCtx<'_, models::v7::CondaPackageDataModel<'static>>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError> {
        match self {
            DeserializablePackageSelectorV7::Conda(selector) => {
                resolve_conda_selector_v7(selector, ctx)
            }
            DeserializablePackageSelectorV7::Pypi(selector) => resolve_pypi_selector(selector, ctx),
        }
    }
}

/// Resolve a V7 conda selector using simplified logic with models:
/// - Binary packages: match by location + subdir (using model's subdir field)
/// - Source packages: match by location + name + variants (using model's variants field)
fn resolve_conda_selector_v7(
    selector: CondaSelectorV7,
    ctx: &mut ResolveCtx<'_, models::v7::CondaPackageDataModel<'static>>,
) -> Result<EnvironmentPackageData, ParseCondaLockError> {
    let CondaSelectorV7 {
        conda,
        name,
        variants,
    } = selector;

    let candidates = ctx
        .conda_url_lookup
        .get(&conda)
        .map_or(&[] as &[usize], Vec::as_slice);

    // Find matching package using model fields
    let package_index = candidates
        .iter()
        .find(|&&idx| {
            let model = &ctx.conda_packages[idx];

            // Name must match if specified
            if let Some(expected_name) = &name {
                if let Some(model_name) = &model.name {
                    if expected_name != model_name.as_ref() {
                        return false;
                    }
                }
            }

            // Check if this is a binary or source package
            // Binary packages have a filename that has an archive extension.
            let is_binary = model.location.file_name().is_some_and(|name| {
                rattler_conda_types::package::ArchiveType::try_from(name).is_some()
            });

            if is_binary {
                // Get the subdir - either from the model or derive from the URL
                let derived_fields = LocationDerivedFields::new(&model.location);
                let subdir = model.subdir.as_deref().or(derived_fields.subdir.as_deref());

                // Must match the current platform's subdir OR be noarch
                if let Some(subdir) = subdir {
                    subdir == ctx.platform.as_str() || subdir == "noarch"
                } else {
                    false
                }
            } else {
                // Source package - match listed variants
                for (expected_key, expected_value) in &variants {
                    if model
                        .variants
                        .get(expected_key)
                        .is_none_or(|v| v != expected_value)
                    {
                        return false;
                    }
                }
                true
            }
        })
        .copied();

    let package_index = package_index.ok_or_else(|| {
        ParseCondaLockError::MissingPackage(
            ctx.environment_name.to_string(),
            ctx.platform,
            conda.clone(),
        )
    })?;

    Ok(EnvironmentPackageData::Conda(package_index))
}

/// Resolve conda selector for V6 (works with models)
fn resolve_conda_selector_v6_models(
    selector: CondaSelectorV6,
    ctx: &mut ResolveCtx<'_, v6::CondaPackageDataModel<'static>>,
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

    let package_index = resolve_conda_selector_v6(
        ctx.conda_packages.as_slice(),
        candidates,
        name.as_ref(),
        version.as_ref(),
        build.as_deref(),
        subdir.as_deref(),
    );

    let package_index = package_index.ok_or_else(|| {
        ParseCondaLockError::MissingPackage(
            ctx.environment_name.to_string(),
            ctx.platform,
            conda.clone(),
        )
    })?;

    Ok(EnvironmentPackageData::Conda(package_index))
}

/// Resolve conda selector for V5 and earlier (works with final CondaPackageData)
/// This merges duplicate records when only location information is available.
fn resolve_conda_selector_v5(
    selector: CondaSelectorV6,
    ctx: &mut ResolveCtx<'_, CondaPackageData>,
) -> Result<EnvironmentPackageData, ParseCondaLockError> {
    let CondaSelectorV6 { conda, .. } = selector;

    let candidates = ctx
        .conda_url_lookup
        .get(&conda)
        .map_or(&[] as &[usize], Vec::as_slice);

    // Filter to platform-specific packages (or noarch)
    let mut indices: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&idx| {
            let Some(binary) = ctx.conda_packages[idx].as_binary() else {
                return false;
            };
            binary.package_record.subdir.as_str() == ctx.platform.as_str()
                || binary.package_record.subdir.as_str() == "noarch"
        })
        .collect();

    // If no platform-specific packages found, use all candidates
    if indices.is_empty() {
        indices.extend_from_slice(candidates);
    }

    let mut iter = indices.into_iter();
    let first_package_idx = iter.next().ok_or_else(|| {
        ParseCondaLockError::MissingPackage(
            ctx.environment_name.to_string(),
            ctx.platform,
            conda.clone(),
        )
    })?;

    // Merge duplicate records
    let merged_package = iter.fold(
        Cow::Borrowed(&ctx.conda_packages[first_package_idx]),
        |acc, next_package_idx| {
            if let Cow::Owned(merged) = acc.merge(&ctx.conda_packages[next_package_idx]) {
                Cow::Owned(merged)
            } else {
                acc
            }
        },
    );

    let package_index = match merged_package {
        Cow::Borrowed(_) => first_package_idx,
        Cow::Owned(package) => {
            ctx.conda_packages.push(package);
            ctx.conda_packages.len() - 1
        }
    };

    Ok(EnvironmentPackageData::Conda(package_index))
}

/// Modern selector resolution for V6+ using model fields directly.
/// This works with CondaPackageDataModel which has the selector fields available.
fn resolve_conda_selector_v6(
    conda_packages: &[v6::CondaPackageDataModel<'_>],
    candidates: &[usize],
    name: Option<&PackageName>,
    version: Option<&VersionWithSource>,
    build: Option<&str>,
    subdir: Option<&str>,
) -> Option<usize> {
    candidates
        .iter()
        .find(|&&idx| {
            let model = &conda_packages[idx];

            // Check name - compare with model's name field if present
            if let Some(expected_name) = name {
                if let Some(model_name) = &model.name {
                    if expected_name != model_name.as_ref() {
                        return false;
                    }
                }
            }

            // Derive fields from URL if needed
            let derived_fields = LocationDerivedFields::new(&model.location);

            // Check version - model's version field or derive from URL
            if let Some(expected_version) = version {
                let version_matches = match &model.version {
                    Some(v) => expected_version == v.as_ref(),
                    None => derived_fields
                        .version
                        .as_ref()
                        .is_some_and(|v| expected_version == v),
                };
                if !version_matches {
                    return false;
                }
            }

            // Check build - model's build field or derive from URL
            if let Some(expected_build) = build {
                let build_matches = match &model.build {
                    Some(b) => expected_build == &**b,
                    None => derived_fields
                        .build
                        .as_deref()
                        .is_some_and(|b| expected_build == b),
                };
                if !build_matches {
                    return false;
                }
            }

            // Check subdir - model's subdir field or derive from URL
            if let Some(expected_subdir) = subdir {
                let subdir_matches = match &model.subdir {
                    Some(s) => expected_subdir == &**s,
                    None => derived_fields
                        .subdir
                        .as_deref()
                        .is_some_and(|s| expected_subdir == s),
                };
                if !subdir_matches {
                    return false;
                }
            }

            true
        })
        .copied()
}

/// Resolve a PyPI selector into the shared package index while deduplicating
/// environment runtime configurations.
fn resolve_pypi_selector<CP>(
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
                    environment_name: &environment_name,
                    platform,
                    conda_packages: &mut conda_packages,
                    conda_url_lookup: &conda_url_lookup,
                    pypi_url_lookup: &pypi_url_lookup,
                    pypi_runtime_lookup: &mut pypi_runtime_lookup,
                };

                let mut platform_packages = IndexSet::new();
                for selector in selectors {
                    let package = V::resolve_selector(selector, &mut ctx)?;
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
