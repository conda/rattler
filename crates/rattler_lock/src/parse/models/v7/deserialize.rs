//! Deserialization helpers specific to V7 lock file contents.

use std::collections::BTreeMap;

use itertools::Itertools;
use rattler_conda_types::PackageName;
use serde::{de::Error, Deserialize};
use serde_value::Value;

use crate::{
    parse::{models::v7, ParseCondaLockError, V7},
    utils::derived_fields::LocationDerivedFields,
    EnvironmentPackageData, PypiPackageData, UrlOrPath,
};

use super::super::super::deserialize::{
    HasLocation, LockFileVersion, PackageSelector, PypiSelector, ResolveCtx,
};

impl HasLocation for v7::CondaPackageDataModel<'static> {
    fn location(&self) -> &UrlOrPath {
        &self.location
    }
}

/// V7-specific package data (stores models before conversion)
#[allow(clippy::large_enum_variant)]
pub(crate) enum PackageDataV7<'a> {
    Conda(v7::CondaPackageDataModel<'a>),
    Pypi(v7::PypiPackageDataModel<'a>),
}

impl<'de> Deserialize<'de> for PackageDataV7<'static> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[allow(clippy::large_enum_variant)]
        #[serde(untagged)]
        enum RawRecord {
            Conda {
                conda: String,
                #[serde(flatten)]
                extra: BTreeMap<Value, Value>,
            },
            Pypi {
                pypi: String,
                #[serde(flatten)]
                extra: BTreeMap<Value, Value>,
            },
        }

        let record = RawRecord::deserialize(deserializer)?;
        Ok(match record {
            RawRecord::Conda { conda, mut extra } => {
                extra.insert(Value::String(String::from("conda")), Value::String(conda));
                Self::Conda(
                    Value::Map(extra)
                        .deserialize_into()
                        .map_err(D::Error::custom)?,
                )
            }
            RawRecord::Pypi { pypi, mut extra } => {
                extra.insert(Value::String(String::from("pypi")), Value::String(pypi));
                Self::Pypi(
                    Value::Map(extra)
                        .deserialize_into()
                        .map_err(D::Error::custom)?,
                )
            }
        })
    }
}

// V7 selectors - simplified, use variants for source package disambiguation
// Binary packages are unique by location, so no version/build/subdir needed
/// Conda selector representation stored in V7 lock files.
#[derive(Deserialize)]
pub(crate) struct CondaSelectorV7 {
    /// URL or path to the conda artifact referenced by the selector.
    #[serde(rename = "conda")]
    pub(crate) conda: UrlOrPath,
    /// Optional package name used to disambiguate source artifacts.
    pub(crate) name: Option<PackageName>,
    /// Variants recorded in the lock file to disambiguate source packages.
    #[serde(default)]
    pub(crate) variants: BTreeMap<String, crate::VariantValue>,
}

/// Selector used in V7 lock files to reference either a conda or PyPI package.
#[allow(clippy::doc_markdown)]
#[derive(Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum DeserializablePackageSelectorV7 {
    Conda(CondaSelectorV7),
    Pypi(PypiSelector),
}

impl PackageSelector<v7::CondaPackageDataModel<'static>> for DeserializablePackageSelectorV7 {
    fn resolve(
        self,
        ctx: &mut ResolveCtx<'_, v7::CondaPackageDataModel<'static>>,
    ) -> Result<EnvironmentPackageData, ParseCondaLockError> {
        match self {
            DeserializablePackageSelectorV7::Conda(selector) => {
                resolve_conda_selector_v7(selector, ctx)
            }
            DeserializablePackageSelectorV7::Pypi(selector) => {
                super::super::super::deserialize::resolve_pypi_selector(selector, ctx)
            }
        }
    }
}

/// Check if a binary package candidate matches the selector criteria.
fn matches_binary_candidate(
    model: &v7::CondaPackageDataModel<'static>,
    platform: rattler_conda_types::Platform,
) -> bool {
    // Get the subdir - either from the model or derive from the URL
    let derived_fields = LocationDerivedFields::new(&model.location);
    let subdir = model.subdir.as_deref().or(derived_fields.subdir.as_deref());

    // Must match the current platform's subdir OR be noarch
    match subdir {
        Some(subdir) => subdir == platform.as_str() || subdir == "noarch",
        None => false,
    }
}

/// Check if a source package candidate matches the selector criteria.
fn matches_source_candidate(
    model: &v7::CondaPackageDataModel<'static>,
    expected_variants: &std::collections::BTreeMap<String, crate::VariantValue>,
) -> bool {
    // Source package - all expected variants must match
    expected_variants
        .iter()
        .all(|(expected_key, expected_value)| {
            model
                .variants
                .get(expected_key)
                .is_some_and(|v| v == expected_value)
        })
}

/// Check if the package name matches the selector criteria.
fn matches_name(
    model: &v7::CondaPackageDataModel<'static>,
    expected_name: &Option<rattler_conda_types::PackageName>,
) -> bool {
    match (expected_name, &model.name) {
        (Some(expected), Some(model_name)) => expected == model_name.as_ref(),
        (None, _) => true,
        _ => false,
    }
}

/// Resolve a V7 conda selector using simplified logic with models:
/// - Binary packages: match by location + subdir (using model's subdir field)
/// - Source packages: match by location + name + variants (using model's variants field)
fn resolve_conda_selector_v7(
    selector: CondaSelectorV7,
    ctx: &mut ResolveCtx<'_, v7::CondaPackageDataModel<'static>>,
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
            if !matches_name(model, &name) {
                return false;
            }

            // Check if this is a binary or source package
            // Binary packages have a filename that has an archive extension.
            let is_binary = model.location.file_name().is_some_and(|name| {
                rattler_conda_types::package::ArchiveType::try_from(name).is_some()
            });

            if is_binary {
                matches_binary_candidate(model, ctx.platform)
            } else {
                matches_source_candidate(model, &variants)
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

// Implement LockFileVersion for V7
impl LockFileVersion for V7 {
    type PackageData = PackageDataV7<'static>;
    type Selector = DeserializablePackageSelectorV7;
    type CondaPackage = v7::CondaPackageDataModel<'static>;

    fn extract_packages(
        packages: Vec<Self::PackageData>,
    ) -> Result<(Vec<Self::CondaPackage>, Vec<PypiPackageData>), ParseCondaLockError> {
        Ok(packages.into_iter().partition_map(|package| match package {
            PackageDataV7::Conda(model) => itertools::Either::Left(model),
            PackageDataV7::Pypi(model) => itertools::Either::Right(model.into()),
        }))
    }
}
