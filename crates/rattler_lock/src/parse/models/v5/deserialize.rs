use std::borrow::Cow;

use itertools::Itertools;
use serde::{Deserialize, Deserializer};

use crate::{
    parse::{models, ParseCondaLockError, V5},
    CondaPackageData, EnvironmentPackageData, PypiPackageData, UrlOrPath,
};

use super::super::super::deserialize::{
    HasLocation, LockFileVersion, PackageSelector, PypiSelector, ResolveCtx,
};

impl HasLocation for CondaPackageData {
    fn location(&self) -> &UrlOrPath {
        CondaPackageData::location(self)
    }
}

/// V5-specific package data (stores final converted types)
#[allow(clippy::large_enum_variant)]
pub(crate) enum PackageDataV5 {
    Conda(CondaPackageData),
    Pypi(PypiPackageData),
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

// V5 selectors - only have location, no disambiguation fields
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum DeserializablePackageSelectorV5 {
    Conda(CondaSelectorV5),
    Pypi(PypiSelector),
}

#[derive(Deserialize)]
pub(crate) struct CondaSelectorV5 {
    #[serde(rename = "conda")]
    pub(crate) conda: crate::UrlOrPath,
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
            DeserializablePackageSelectorV5::Pypi(selector) => {
                super::super::super::deserialize::resolve_pypi_selector(selector, ctx)
            }
        }
    }
}

/// Resolve conda selector for V5 and earlier (works with final `CondaPackageData`)
/// This merges duplicate records when only location information is available.
fn resolve_conda_selector_v5(
    selector: CondaSelectorV5,
    ctx: &mut ResolveCtx<'_, CondaPackageData>,
) -> Result<EnvironmentPackageData, ParseCondaLockError> {
    let conda = selector.conda;

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

// Implement LockFileVersion for V5
impl LockFileVersion for V5 {
    type PackageData = PackageDataV5;
    type Selector = DeserializablePackageSelectorV5;
    type CondaPackage = CondaPackageData;

    fn extract_packages(
        packages: Vec<Self::PackageData>,
    ) -> Result<(Vec<Self::CondaPackage>, Vec<PypiPackageData>), ParseCondaLockError> {
        Ok(packages.into_iter().partition_map(|package| match package {
            PackageDataV5::Conda(p) => itertools::Either::Left(p),
            PackageDataV5::Pypi(p) => itertools::Either::Right(p),
        }))
    }
}
