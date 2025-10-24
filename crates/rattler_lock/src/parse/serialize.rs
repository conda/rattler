use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashSet},
    marker::PhantomData,
};

use itertools::Itertools;
use pep508_rs::ExtraName;
use rattler_conda_types::{PackageName, Platform};
use serde::{Serialize, Serializer};
use serde_with::{serde_as, SerializeAs};
use url::Url;

use crate::{
    file_format_version::FileFormatVersion,
    parse::{models, V7},
    Channel, CondaBinaryData, CondaPackageData, CondaSourceData, EnvironmentData,
    EnvironmentPackageData, LockFile, LockFileInner, PypiIndexes, PypiPackageData,
    PypiPackageEnvironmentData, SolveOptions, UrlOrPath,
};

#[serde_as]
#[derive(Serialize)]
#[serde(bound(serialize = "V: SerializeAs<PackageData<'a>>"))]
struct SerializableLockFile<'a, V> {
    version: FileFormatVersion,
    environments: BTreeMap<&'a String, SerializableEnvironment<'a>>,
    #[serde_as(as = "Vec<V>")]
    packages: Vec<PackageData<'a>>,
    #[serde(skip)]
    _version: PhantomData<V>,
}

#[derive(Serialize)]
struct SerializableEnvironment<'a> {
    channels: &'a [Channel],
    #[serde(flatten)]
    indexes: Option<&'a PypiIndexes>,
    #[serde(default, skip_serializing_if = "crate::utils::serde::is_default")]
    options: SolveOptions,
    packages: BTreeMap<Platform, Vec<SerializablePackageSelector<'a>>>,
}

impl<'a> SerializableEnvironment<'a> {
    fn from_environment<E: serde::ser::Error>(
        inner: &'a LockFileInner,
        env_data: &'a EnvironmentData,
        used_conda_packages: &HashSet<usize>,
        used_pypi_packages: &HashSet<usize>,
    ) -> Result<Self, E> {
        let mut packages = BTreeMap::new();

        for (platform, platform_packages) in &env_data.packages {
            let mut selectors = Vec::new();
            for &package_data in platform_packages {
                let selector = SerializablePackageSelector::from_lock_file(
                    inner,
                    package_data,
                    *platform,
                    used_conda_packages,
                    used_pypi_packages,
                )?;
                selectors.push(selector);
            }
            selectors.sort();
            packages.insert(*platform, selectors);
        }

        Ok(SerializableEnvironment {
            channels: &env_data.channels,
            indexes: env_data.indexes.as_ref(),
            options: env_data.options.clone(),
            packages,
        })
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Serialize, Eq, PartialEq)]
#[serde(untagged)]
enum SerializablePackageDataV7<'a> {
    Conda(models::v7::CondaPackageDataModel<'a>),
    Pypi(models::v7::PypiPackageDataModel<'a>),
}

impl<'a> From<PackageData<'a>> for SerializablePackageDataV7<'a> {
    fn from(package: PackageData<'a>) -> Self {
        match package {
            PackageData::Conda(p) => Self::Conda(p.into()),
            PackageData::Pypi(p) => Self::Pypi(p.into()),
        }
    }
}

#[derive(Serialize, Eq, PartialEq)]
#[serde(untagged, rename_all = "snake_case")]
enum SerializablePackageSelector<'a> {
    Conda {
        conda: &'a UrlOrPath,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<&'a PackageName>,
        // V7: variants for source package disambiguation
        // Binary packages have empty variants map
        #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
        variants: BTreeMap<String, crate::VariantValue>,
    },
    Pypi {
        pypi: &'a UrlOrPath,
        #[serde(skip_serializing_if = "BTreeSet::is_empty")]
        extras: &'a BTreeSet<ExtraName>,
    },
}

impl<'a> SerializablePackageSelector<'a> {
    fn from_lock_file<E: serde::ser::Error>(
        inner: &'a LockFileInner,
        package: EnvironmentPackageData,
        platform: Platform,
        used_conda_packages: &HashSet<usize>,
        used_pypi_packages: &HashSet<usize>,
    ) -> Result<Self, E> {
        match package {
            EnvironmentPackageData::Conda(idx) => Self::from_conda(
                inner,
                &inner.conda_packages[idx],
                platform,
                used_conda_packages,
            ),
            EnvironmentPackageData::Pypi(pkg_data_idx, env_data_idx) => Ok(Self::from_pypi(
                inner,
                &inner.pypi_packages[pkg_data_idx],
                &inner.pypi_environment_package_data[env_data_idx],
                used_pypi_packages,
            )),
        }
    }

    fn from_conda<E: serde::ser::Error>(
        inner: &'a LockFileInner,
        package: &'a CondaPackageData,
        platform: Platform,
        used_conda_packages: &HashSet<usize>,
    ) -> Result<Self, E> {
        match package {
            CondaPackageData::Binary(_) => {
                Self::build_binary_selector(inner, package, platform, used_conda_packages)
            }
            CondaPackageData::Source(_) => {
                Self::build_source_selector(inner, package, used_conda_packages)
            }
        }
    }

    fn from_pypi(
        _inner: &'a LockFileInner,
        package: &'a PypiPackageData,
        env: &'a PypiPackageEnvironmentData,
        _used_pypi_packages: &HashSet<usize>,
    ) -> Self {
        Self::Pypi {
            pypi: &package.location,
            extras: &env.extras,
        }
    }
}

/// Determine the smallest set of variant key/value pairs that uniquely identify
/// `target` among the remaining conflicting source packages.
fn select_minimal_variant_keys<'a>(
    target: &'a CondaSourceData,
    mut conflicts: Vec<&'a CondaSourceData>,
) -> Option<BTreeMap<String, crate::VariantValue>> {
    let mut selected = BTreeMap::new();

    // Keep picking discriminating keys until no conflicting packages remain.
    while !conflicts.is_empty() {
        let mut best: Option<(String, crate::VariantValue, Vec<&CondaSourceData>)> = None;

        for (key, value) in &target.variants {
            if selected.contains_key(key) {
                continue;
            }

            // Restrict conflicts to those that share the current key/value combo.
            let matching: Vec<&CondaSourceData> = conflicts
                .iter()
                .copied()
                .filter(|candidate| candidate.variants.get(key) == Some(value))
                .collect();

            if matching.len() == conflicts.len() {
                continue;
            }

            match &best {
                Some((best_key, _best_value, best_matching)) => {
                    if matching.len() < best_matching.len()
                        || (matching.len() == best_matching.len() && key < best_key)
                    {
                        best = Some((key.clone(), value.clone(), matching));
                    }
                }
                None => best = Some((key.clone(), value.clone(), matching)),
            }
        }

        // If no new key can reduce the conflict set, disambiguation fails.
        let (key, value, reduced) = best?;
        selected.insert(key, value);
        conflicts = reduced;
    }

    Some(selected)
}

impl<'a> SerializablePackageSelector<'a> {
    /// Serialize a binary package selector, erroring if multiple binaries remain.
    fn build_binary_selector<E: serde::ser::Error>(
        inner: &'a LockFileInner,
        package: &'a CondaPackageData,
        platform: Platform,
        used_conda_packages: &HashSet<usize>,
    ) -> Result<Self, E> {
        let binary = package
            .as_binary()
            .expect("build_binary_selector should only be called for binary packages");

        // Gather all binary packages at the same location that apply to the platform.
        let candidates: Vec<&CondaBinaryData> = inner
            .conda_packages
            .iter()
            .enumerate()
            .filter(|(idx, p)| {
                used_conda_packages.contains(idx) && p.location() == package.location()
            })
            .filter_map(|(_, p)| p.as_binary())
            .filter(|candidate| {
                candidate.package_record.subdir == platform.as_str()
                    || candidate.package_record.subdir == "noarch"
            })
            .collect();

        // The target binary must still be present after filtering.
        if candidates.is_empty()
            || !candidates
                .iter()
                .any(|candidate| std::ptr::eq(*candidate, binary))
        {
            return Err(E::custom(format!(
                "Failed to locate binary package '{}' for platform '{}'.",
                package.location(),
                platform.as_str()
            )));
        }

        // If more than one candidate survives, we cannot serialize an unambiguous selector.
        if candidates.len() > 1 {
            return Err(E::custom(format!(
                "Failed to disambiguate binary packages at location '{}' for platform '{}'. \
                 Multiple packages share the same location and subdir.",
                package.location(),
                platform.as_str()
            )));
        }

        Ok(Self::Conda {
            conda: package.location(),
            name: None,
            variants: BTreeMap::new(),
        })
    }

    /// Serialize a source selector, emitting the minimal variant subset needed.
    fn build_source_selector<E: serde::ser::Error>(
        inner: &'a LockFileInner,
        package: &'a CondaPackageData,
        used_conda_packages: &HashSet<usize>,
    ) -> Result<Self, E> {
        let source = package
            .as_source()
            .expect("build_source_selector should only be called for source packages");

        // Collect every source package referenced from the same location.
        let similar_sources: Vec<&CondaSourceData> = inner
            .conda_packages
            .iter()
            .enumerate()
            .filter(|(idx, p)| {
                used_conda_packages.contains(idx) && p.location() == package.location()
            })
            .filter_map(|(_, p)| p.as_source())
            .collect();

        // Include the package name if multiple names exist at this location.
        let include_name = similar_sources
            .iter()
            .any(|candidate| !std::ptr::eq(*candidate, source) && candidate.name != source.name);

        let mut conflicts: Vec<&CondaSourceData> = similar_sources
            .into_iter()
            .filter(|candidate| !std::ptr::eq(*candidate, source))
            .collect();

        if include_name {
            conflicts.retain(|candidate| candidate.name == source.name);
        }

        let variants = select_minimal_variant_keys(source, conflicts.clone()).ok_or_else(|| {
            // Build a list of all conflicting packages for the error message
            let mut all_packages = vec![source];
            all_packages.extend(conflicts.iter().copied());

            let package_details: Vec<String> = all_packages
                .iter()
                .map(|pkg| {
                    let version_str = pkg.version
                        .as_ref()
                        .map(|v| format!(" (version: {})", v))
                        .unwrap_or_default();
                    let variants_str = if pkg.variants.is_empty() {
                        String::new()
                    } else {
                        let variant_pairs: Vec<String> = pkg.variants
                            .iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect();
                        format!(" [variants: {}]", variant_pairs.join(", "))
                    };
                    format!("  - {}{}{}", pkg.name.as_normalized(), version_str, variants_str)
                })
                .collect();

            E::custom(format!(
                "Failed to disambiguate source packages at location '{}'. \
                 Multiple source packages exist but cannot be distinguished without variant information. \
                 This typically occurs when converting from lock file format V6 to V7.\n\
                 Conflicting packages:\n{}",
                package.location(),
                package_details.join("\n")
            ))
        })?;

        Ok(Self::Conda {
            conda: package.location(),
            name: include_name.then_some(package.name()),
            variants,
        })
    }
}

impl PartialOrd for SerializablePackageSelector<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SerializablePackageSelector<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (
                SerializablePackageSelector::Conda { .. },
                SerializablePackageSelector::Pypi { .. },
            ) => {
                // Sort conda packages before pypi packages
                Ordering::Less
            }
            (
                SerializablePackageSelector::Pypi { .. },
                SerializablePackageSelector::Conda { .. },
            ) => {
                // Sort Pypi packages after conda packages
                Ordering::Greater
            }
            (
                SerializablePackageSelector::Conda {
                    conda: a,
                    name: name_a,
                    variants: variants_a,
                },
                SerializablePackageSelector::Conda {
                    conda: b,
                    name: name_b,
                    variants: variants_b,
                },
            ) => compare_url_by_location(a, b)
                .then_with(|| name_a.cmp(name_b))
                .then_with(|| variants_a.cmp(variants_b)),
            (
                SerializablePackageSelector::Pypi { pypi: a, .. },
                SerializablePackageSelector::Pypi { pypi: b, .. },
            ) => compare_url_by_location(a, b),
        }
    }
}

/// First sort packages just by their filename. Since most of the time the urls
/// end in the packages filename this causes the urls to be sorted by package
/// name.
fn compare_url_by_filename(a: &Url, b: &Url) -> Ordering {
    if let (Some(a), Some(b)) = (
        a.path_segments()
            .and_then(Iterator::last)
            .map(str::to_lowercase),
        b.path_segments()
            .and_then(Iterator::last)
            .map(str::to_lowercase),
    ) {
        match a.cmp(&b) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }

    // Otherwise just sort by their full URL
    a.cmp(b)
}

fn compare_url_by_location(a: &UrlOrPath, b: &UrlOrPath) -> Ordering {
    match (a, b) {
        (UrlOrPath::Url(a), UrlOrPath::Url(b)) => compare_url_by_filename(a, b),
        (UrlOrPath::Url(_), UrlOrPath::Path(_)) => Ordering::Less,
        (UrlOrPath::Path(_), UrlOrPath::Url(_)) => Ordering::Greater,
        (UrlOrPath::Path(a), UrlOrPath::Path(b)) => a.as_str().cmp(b.as_str()),
    }
}

impl<'a> SerializeAs<PackageData<'a>> for V7 {
    fn serialize_as<S>(source: &PackageData<'a>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializablePackageDataV7::from(*source).serialize(serializer)
    }
}

impl Serialize for LockFile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let inner = self.inner.as_ref();

        // Determine the package indexes that are used in the lock-file.
        let mut used_conda_packages = HashSet::new();
        let mut used_pypi_packages = HashSet::new();
        for env in inner.environments.iter() {
            for packages in env.packages.values() {
                for package in packages {
                    match package {
                        EnvironmentPackageData::Conda(idx) => {
                            used_conda_packages.insert(*idx);
                        }
                        EnvironmentPackageData::Pypi(pkg_idx, _env_idx) => {
                            used_pypi_packages.insert(*pkg_idx);
                        }
                    }
                }
            }
        }

        // Collect all environments
        let mut environments = BTreeMap::new();
        for (name, env_idx) in &inner.environment_lookup {
            let env = SerializableEnvironment::from_environment(
                inner,
                &inner.environments[*env_idx],
                &used_conda_packages,
                &used_pypi_packages,
            )?;
            environments.insert(name, env);
        }

        // Get all packages.
        let conda_packages = inner
            .conda_packages
            .iter()
            .enumerate()
            .filter(|(idx, _)| used_conda_packages.contains(idx))
            .map(|(_, p)| PackageData::Conda(p));

        let pypi_packages = inner
            .pypi_packages
            .iter()
            .enumerate()
            .filter(|(idx, _)| used_pypi_packages.contains(idx))
            .map(|(_, p)| PackageData::Pypi(p));

        // Sort the packages in a deterministic order. See [`SerializablePackageData`]
        // for more information.
        let packages = itertools::chain!(conda_packages, pypi_packages).sorted();

        // Always serialize using the LATEST version
        let raw = SerializableLockFile {
            version: FileFormatVersion::LATEST,
            environments,
            packages: packages.collect(),
            _version: PhantomData::<V7>,
        };

        raw.serialize(serializer)
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum PackageData<'a> {
    Conda(&'a CondaPackageData),
    Pypi(&'a PypiPackageData),
}

impl PackageData<'_> {
    fn source_name(&self) -> &str {
        match self {
            PackageData::Conda(p) => p.name().as_source(),
            PackageData::Pypi(p) => p.name.as_ref(),
        }
    }
}

impl PartialOrd<Self> for PackageData<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageData<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        use PackageData::{Conda, Pypi};
        self.source_name()
            .cmp(other.source_name())
            .then_with(|| match (self, other) {
                (Conda(a), Conda(b)) => a.cmp(b),
                (Pypi(a), Pypi(b)) => a.cmp(b),
                (Pypi(_), _) => Ordering::Less,
                (_, Pypi(_)) => Ordering::Greater,
            })
    }
}

impl Serialize for CondaPackageData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializablePackageDataV7::Conda(models::v7::CondaPackageDataModel::from(self))
            .serialize(serializer)
    }
}

impl Serialize for PypiPackageData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializablePackageDataV7::Pypi(models::v7::PypiPackageDataModel::from(self))
            .serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::select_minimal_variant_keys;
    use crate::{CondaSourceData, UrlOrPath, VariantValue};
    use rattler_conda_types::PackageName;
    use std::collections::BTreeMap;
    use std::str::FromStr;
    use url::Url;

    fn make_source(
        name: &str,
        location: &str,
        variants: BTreeMap<String, VariantValue>,
    ) -> CondaSourceData {
        CondaSourceData {
            name: PackageName::from_str(name).unwrap(),
            version: None,
            location: UrlOrPath::from(Url::parse(location).unwrap()),
            variants,
            depends: Vec::new(),
            constrains: Vec::new(),
            experimental_extra_depends: BTreeMap::new(),
            license: None,
            purls: None,
            sources: BTreeMap::new(),
            input: None,
            package_build_source: None,
            python_site_packages_path: None,
        }
    }

    /// Only one variant key differs (python version), so a single key should be selected.
    #[test]
    fn selects_single_variant_key() {
        let mut target_variants = BTreeMap::new();
        target_variants.insert(
            "python".to_string(),
            VariantValue::String("3.10".to_string()),
        );
        let target = make_source("foo", "https://example.org/pkg", target_variants);

        let mut other_variants = BTreeMap::new();
        other_variants.insert(
            "python".to_string(),
            VariantValue::String("3.11".to_string()),
        );
        let other = make_source("foo", "https://example.org/pkg", other_variants);

        let selected =
            select_minimal_variant_keys(&target, vec![&other]).expect("should disambiguate");
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected.get("python"),
            Some(&VariantValue::String("3.10".to_string()))
        );
    }

    /// Two variants (`cuda` + `blas_impl`) are necessary to eliminate all conflicts.
    #[test]
    fn selects_multiple_variant_keys_when_needed() {
        let mut target_variants = BTreeMap::new();
        target_variants.insert(
            "python".to_string(),
            VariantValue::String("3.10".to_string()),
        );
        target_variants.insert("cuda".to_string(), VariantValue::String("11.8".to_string()));
        target_variants.insert(
            "blas_impl".to_string(),
            VariantValue::String("openblas".to_string()),
        );
        let target = make_source("foo", "https://example.org/pkg", target_variants);

        let mut other1_variants = BTreeMap::new();
        other1_variants.insert(
            "python".to_string(),
            VariantValue::String("3.10".to_string()),
        );
        other1_variants.insert("cuda".to_string(), VariantValue::String("11.2".to_string()));
        other1_variants.insert(
            "blas_impl".to_string(),
            VariantValue::String("openblas".to_string()),
        );
        let other1 = make_source("foo", "https://example.org/pkg", other1_variants);

        let mut other2_variants = BTreeMap::new();
        other2_variants.insert(
            "python".to_string(),
            VariantValue::String("3.10".to_string()),
        );
        other2_variants.insert("cuda".to_string(), VariantValue::String("11.8".to_string()));
        other2_variants.insert(
            "blas_impl".to_string(),
            VariantValue::String("mkl".to_string()),
        );
        let other2 = make_source("foo", "https://example.org/pkg", other2_variants);

        let selected = select_minimal_variant_keys(&target, vec![&other1, &other2])
            .expect("should disambiguate");
        assert_eq!(selected.len(), 2);
        assert_eq!(
            selected.get("cuda"),
            Some(&VariantValue::String("11.8".to_string()))
        );
        assert_eq!(
            selected.get("blas_impl"),
            Some(&VariantValue::String("openblas".to_string()))
        );
    }

    /// Identical variant maps cannot be distinguished, so the helper must fail.
    #[test]
    fn returns_none_when_variants_insufficient() {
        let mut target_variants = BTreeMap::new();
        target_variants.insert(
            "python".to_string(),
            VariantValue::String("3.10".to_string()),
        );
        let target = make_source("foo", "https://example.org/pkg", target_variants.clone());
        let other = make_source("foo", "https://example.org/pkg", target_variants);

        assert!(
            select_minimal_variant_keys(&target, vec![&other]).is_none(),
            "expected disambiguation failure"
        );
    }
}
