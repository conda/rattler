//! Deserialization helpers specific to V5 lock file contents.

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
/// Selector used in V5 lock files to reference either a conda or PyPI package.
#[allow(clippy::doc_markdown)]
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum DeserializablePackageSelectorV5 {
    Conda(CondaSelectorV5),
    Pypi(PypiSelector),
}

/// Minimal conda selector available in V5 lock files (location only).
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

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::Platform;
    use url::Url;

    /// Helper to create a test binary package
    fn test_binary_package(location: UrlOrPath, subdir: &str) -> CondaPackageData {
        use crate::conda::CondaBinaryData;
        use rattler_conda_types::{PackageRecord, Version};
        use std::str::FromStr;

        CondaPackageData::Binary(CondaBinaryData {
            package_record: PackageRecord {
                subdir: subdir.to_string(),
                ..PackageRecord::new(
                    "test-package".parse().unwrap(),
                    Version::from_str("1.0.0").unwrap(),
                    "0".to_string(),
                )
            },
            location,
            file_name: "package.tar.bz2".to_string(),
            channel: None,
        })
    }

    /// Tests that V5 selectors correctly filter packages by platform subdir.
    ///
    /// V5 selectors only contain location information, so they must match by
    /// comparing the package's subdir field against the target platform.
    #[test]
    fn test_conda_selector_v5_matches_platform_subdir() {
        let location = UrlOrPath::Url(Url::parse("https://example.com/package.tar.bz2").unwrap());
        let selector = CondaSelectorV5 {
            conda: location.clone(),
        };

        let linux64_pkg = test_binary_package(location.clone(), "linux-64");
        let win64_pkg = test_binary_package(location.clone(), "win-64");

        let mut packages = vec![linux64_pkg, win64_pkg];
        let mut lookup = fxhash::FxHashMap::default();
        lookup.insert(location.clone(), vec![0, 1]);

        let mut ctx = ResolveCtx {
            environment_name: "test",
            platform: Platform::Linux64,
            conda_packages: &mut packages,
            conda_url_lookup: &lookup,
            pypi_url_lookup: &fxhash::FxHashMap::default(),
            pypi_runtime_lookup: &mut indexmap::IndexSet::new(),
        };

        let result = resolve_conda_selector_v5(selector, &mut ctx);
        assert!(result.is_ok());
        // Should resolve to the linux-64 package (index 0)
        assert_eq!(result.unwrap(), EnvironmentPackageData::Conda(0));
    }

    /// Tests that V5 selectors can match noarch packages when platform-specific is unavailable.
    ///
    /// When no packages match the target platform's subdir, V5 falls back to
    /// matching any available packages at the location, including noarch.
    #[test]
    fn test_conda_selector_v5_noarch_fallback() {
        let location = UrlOrPath::Url(Url::parse("https://example.com/package.tar.bz2").unwrap());
        let selector = CondaSelectorV5 {
            conda: location.clone(),
        };

        let noarch_pkg = test_binary_package(location.clone(), "noarch");

        let mut packages = vec![noarch_pkg];
        let mut lookup = fxhash::FxHashMap::default();
        lookup.insert(location.clone(), vec![0]);

        let mut ctx = ResolveCtx {
            environment_name: "test",
            platform: Platform::Linux64,
            conda_packages: &mut packages,
            conda_url_lookup: &lookup,
            pypi_url_lookup: &fxhash::FxHashMap::default(),
            pypi_runtime_lookup: &mut indexmap::IndexSet::new(),
        };

        let result = resolve_conda_selector_v5(selector, &mut ctx);
        assert!(result.is_ok());
    }

    /// Tests that V5 selectors return an error when the package location is not found.
    ///
    /// When a selector references a package location that doesn't exist in the
    /// lock file's package lookup, the resolution must fail with a clear error.
    #[test]
    fn test_conda_selector_v5_missing_package() {
        let location = UrlOrPath::Url(Url::parse("https://example.com/missing.tar.bz2").unwrap());
        let selector = CondaSelectorV5 {
            conda: location.clone(),
        };

        let mut packages = vec![];
        let lookup = fxhash::FxHashMap::default();

        let mut ctx = ResolveCtx {
            environment_name: "test",
            platform: Platform::Linux64,
            conda_packages: &mut packages,
            conda_url_lookup: &lookup,
            pypi_url_lookup: &fxhash::FxHashMap::default(),
            pypi_runtime_lookup: &mut indexmap::IndexSet::new(),
        };

        let result = resolve_conda_selector_v5(selector, &mut ctx);
        assert!(result.is_err());
        assert!(matches!(
            result.err(),
            Some(ParseCondaLockError::MissingPackage(_, _, _))
        ));
    }
}
