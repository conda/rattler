use itertools::Itertools;
use rattler_conda_types::{PackageName, VersionWithSource};
use serde::Deserialize;

use crate::{
    parse::{models::v6, ParseCondaLockError, V6},
    utils::derived_fields::LocationDerivedFields,
    EnvironmentPackageData, PypiPackageData, UrlOrPath,
};

use super::super::super::deserialize::{
    HasLocation, LockFileVersion, PackageSelector, PypiSelector, ResolveCtx,
};

impl HasLocation for v6::CondaPackageDataModel<'static> {
    fn location(&self) -> &UrlOrPath {
        &self.location
    }
}

/// V6-specific package data (stores models before conversion)
#[derive(Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum PackageDataV6<'a> {
    Conda(v6::CondaPackageDataModel<'a>),
    Pypi(v6::PypiPackageDataModel<'a>),
}

#[derive(Deserialize)]
pub(crate) struct CondaSelectorV6 {
    #[serde(rename = "conda")]
    pub(crate) conda: UrlOrPath,
    pub(crate) name: Option<PackageName>,
    pub(crate) version: Option<VersionWithSource>,
    pub(crate) build: Option<String>,
    pub(crate) subdir: Option<String>,
}

impl CondaSelectorV6 {
    /// Resolve this selector to a package index by matching against V6 models.
    ///
    /// This works with `CondaPackageDataModel` which has the selector fields available,
    /// and will derive missing fields (version, build, subdir) from the package URL
    /// when they are not explicitly present in the model.
    pub(crate) fn resolve(
        &self,
        conda_packages: &[v6::CondaPackageDataModel<'_>],
        candidates: &[usize],
    ) -> Option<usize> {
        candidates
            .iter()
            .find(|&&idx| {
                let model = &conda_packages[idx];

                // Check name - compare with model's name field if present
                if let Some(expected_name) = &self.name {
                    if let Some(model_name) = &model.name {
                        if expected_name != model_name.as_ref() {
                            return false;
                        }
                    }
                }

                // Derive fields from URL if needed
                let derived_fields = LocationDerivedFields::new(&model.location);

                // Check version - model's version field or derive from URL
                if let Some(expected_version) = &self.version {
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
                if let Some(expected_build) = &self.build {
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
                if let Some(expected_subdir) = &self.subdir {
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
}

#[derive(Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum DeserializablePackageSelectorV6 {
    Conda(CondaSelectorV6),
    Pypi(PypiSelector),
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
            DeserializablePackageSelectorV6::Pypi(selector) => {
                super::super::super::deserialize::resolve_pypi_selector(selector, ctx)
            }
        }
    }
}

/// Resolve conda selector for V6 (works with models)
fn resolve_conda_selector_v6_models(
    selector: CondaSelectorV6,
    ctx: &mut ResolveCtx<'_, v6::CondaPackageDataModel<'static>>,
) -> Result<EnvironmentPackageData, ParseCondaLockError> {
    let candidates = ctx
        .conda_url_lookup
        .get(&selector.conda)
        .map_or(&[] as &[usize], Vec::as_slice);

    let package_index = selector.resolve(ctx.conda_packages.as_slice(), candidates);

    let package_index = package_index.ok_or_else(|| {
        ParseCondaLockError::MissingPackage(
            ctx.environment_name.to_string(),
            ctx.platform,
            selector.conda.clone(),
        )
    })?;

    Ok(EnvironmentPackageData::Conda(package_index))
}

// Implement LockFileVersion for V6
impl LockFileVersion for V6 {
    type PackageData = PackageDataV6<'static>;
    type Selector = DeserializablePackageSelectorV6;
    type CondaPackage = v6::CondaPackageDataModel<'static>;

    fn extract_packages(
        packages: Vec<Self::PackageData>,
    ) -> Result<(Vec<Self::CondaPackage>, Vec<PypiPackageData>), ParseCondaLockError> {
        Ok(packages.into_iter().partition_map(|package| match package {
            PackageDataV6::Conda(model) => itertools::Either::Left(model),
            PackageDataV6::Pypi(model) => itertools::Either::Right(model.into()),
        }))
    }
}
