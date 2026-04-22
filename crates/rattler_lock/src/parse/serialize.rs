use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
    marker::PhantomData,
};

use itertools::Itertools;
use serde::{Serialize, Serializer};
use serde_with::{serde_as, SerializeAs};

use crate::{
    file_format_version::FileFormatVersion,
    parse::{models::v7, models::v7::PackageSelector, V7},
    Channel, CondaPackageData, EnvironmentData, LockFile, LockFileInner, LockedPackage,
    PackageIndex, PlatformData, PypiIndexes, PypiPackageData, SelectorId, SolveOptions,
};

fn selector_ids_to_package_selectors(ids: Vec<SelectorId>) -> Vec<PackageSelector> {
    ids.iter().map(PackageSelector::from_selector_id).collect()
}

#[serde_as]
#[derive(Serialize)]
#[serde(bound(serialize = "V: SerializeAs<PackageData<'a>>"))]
struct SerializableLockFile<'a, V> {
    version: FileFormatVersion,
    platforms: Vec<SerializablePlatform<'a>>,
    environments: BTreeMap<&'a String, SerializableEnvironment<'a>>,
    #[serde_as(as = "Vec<V>")]
    packages: Vec<PackageData<'a>>,
    #[serde(skip)]
    _version: PhantomData<V>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SerializablePlatform<'a> {
    name: &'a str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subdir: Option<&'static str>,
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    virtual_packages: &'a [String],
}

impl<'a> SerializablePlatform<'a> {
    fn from_platform(platform: &'a PlatformData) -> Self {
        let subdir = (platform.subdir.as_str() != platform.name.as_str())
            .then_some(platform.subdir.as_str());
        Self {
            name: platform.name.as_str(),
            subdir,
            virtual_packages: &platform.virtual_packages,
        }
    }
}

#[derive(Serialize)]
struct SerializableEnvironment<'a> {
    channels: &'a [Channel],
    #[serde(flatten)]
    indexes: Option<&'a PypiIndexes>,
    #[serde(default, skip_serializing_if = "crate::utils::serde::is_default")]
    options: SolveOptions,
    packages: BTreeMap<String, Vec<SerializablePackageSelector>>,
}

impl<'a> SerializableEnvironment<'a> {
    fn from_environment(inner: &'a LockFileInner, env_data: &'a EnvironmentData) -> Self {
        SerializableEnvironment {
            channels: &env_data.channels,
            indexes: env_data.indexes.as_ref(),
            options: env_data.options.clone(),
            packages: env_data
                .packages
                .iter()
                .map(|(platform, packages)| {
                    let platform_name = inner
                        .platforms
                        .get(platform.0)
                        .expect("Platform indices are valid")
                        .name
                        .to_string();
                    (
                        platform_name,
                        packages
                            .iter()
                            .map(|handle| {
                                SerializablePackageSelector::from_lock_file(inner, handle.index)
                            })
                            .sorted()
                            .collect(),
                    )
                })
                .collect(),
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Serialize, Eq, PartialEq)]
#[serde(untagged)]
enum SerializablePackageDataV7<'a> {
    Conda(v7::CondaPackageDataModel<'a>),
    Source(v7::SourcePackageDataModel<'a>),
    Pypi(v7::PypiPackageDataModel<'a>),
}

impl<'a> From<PackageData<'a>> for SerializablePackageDataV7<'a> {
    fn from(package: PackageData<'a>) -> Self {
        match package.package {
            LockedPackage::Conda(CondaPackageData::Binary(binary)) => {
                Self::Conda(binary.as_ref().into())
            }
            LockedPackage::Conda(CondaPackageData::Source(source)) => {
                let mut model = v7::SourcePackageDataModel::from(source.as_ref());
                model.build_packages = selector_ids_to_package_selectors(package.build_packages);
                model.host_packages = selector_ids_to_package_selectors(package.host_packages);
                Self::Source(model)
            }
            LockedPackage::Pypi(p) => {
                let mut model = v7::PypiPackageDataModel::from(p);
                model.build_packages = selector_ids_to_package_selectors(package.build_packages);
                model.host_packages = selector_ids_to_package_selectors(package.host_packages);
                Self::Pypi(model)
            }
        }
    }
}

/// Package selector for V7+ environments.
///
/// For V7+, each package variant is uniquely identified by its
/// [`LockedPackage::selector_id`](crate::LockedPackage::selector_id) string,
/// stored under the appropriate YAML key (`conda`, `conda_source`, or `pypi`).
#[derive(Serialize, Eq, PartialEq)]
#[serde(untagged, rename_all = "snake_case")]
enum SerializablePackageSelector {
    Conda { conda: String },
    CondaSource { conda_source: String },
    Pypi { pypi: String },
}

impl SerializablePackageSelector {
    fn from_lock_file(inner: &LockFileInner, package: PackageIndex) -> Self {
        let pkg = &inner.packages[package.0];
        let id = SelectorId::new(pkg).as_str().to_string();
        match pkg {
            LockedPackage::Conda(CondaPackageData::Binary(_)) => Self::Conda { conda: id },
            LockedPackage::Conda(CondaPackageData::Source(_)) => {
                Self::CondaSource { conda_source: id }
            }
            LockedPackage::Pypi(_) => Self::Pypi { pypi: id },
        }
    }

    fn id(&self) -> &str {
        match self {
            Self::Conda { conda } => conda,
            Self::CondaSource { conda_source } => conda_source,
            Self::Pypi { pypi } => pypi,
        }
    }
}

impl PartialOrd for SerializablePackageSelector {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SerializablePackageSelector {
    fn cmp(&self, other: &Self) -> Ordering {
        fn type_order(selector: &SerializablePackageSelector) -> u8 {
            match selector {
                SerializablePackageSelector::Conda { .. } => 0,
                SerializablePackageSelector::CondaSource { .. } => 1,
                SerializablePackageSelector::Pypi { .. } => 2,
            }
        }

        type_order(self)
            .cmp(&type_order(other))
            .then_with(|| self.id().cmp(other.id()))
    }
}

impl<'a> SerializeAs<PackageData<'a>> for V7 {
    fn serialize_as<S>(source: &PackageData<'a>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializablePackageDataV7::from(source.clone()).serialize(serializer)
    }
}

impl Serialize for LockFile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let inner = self.inner.as_ref();

        // Determine the package indexes that are used in the lock-file.
        let used_packages: HashSet<PackageIndex> = inner
            .environments
            .iter()
            .flat_map(|env| env.packages.values())
            .flat_map(|packages| packages.iter().map(|handle| handle.index))
            .collect();

        // Collect all environments
        let environments = inner
            .environment_lookup
            .iter()
            .map(|(name, env_idx)| {
                (
                    name,
                    SerializableEnvironment::from_environment(
                        inner,
                        &inner.environments[env_idx.0],
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();

        // Get all packages, deduplicating binary packages by location.
        // V7 identifies binary packages by URL uniquely, so we deduplicate here
        // to handle older formats that may have had duplicate entries.
        // Source packages are NOT deduplicated because they use SourceIdentifier
        // which includes a hash to distinguish different configurations at the same location.
        let mut seen_binary_locations = HashSet::new();
        let packages = inner
            .packages
            .iter()
            .enumerate()
            .filter(|(index, _)| used_packages.contains(&PackageIndex(*index)))
            .filter_map(|(_, p)| {
                let source_data = match p {
                    LockedPackage::Conda(conda) => match conda {
                        CondaPackageData::Binary(binary) => seen_binary_locations
                            .insert(binary.location.clone())
                            .then_some(None),
                        CondaPackageData::Source(source) => Some(Some(&source.source_data)),
                    },
                    LockedPackage::Pypi(pypi) => {
                        let source_data = pypi.as_source().map(|s| &s.source_data);
                        Some(source_data)
                    }
                };
                if let Some(source_data) = source_data {
                    let (build_packages, host_packages) = source_data.map_or_else(
                        || (Vec::new(), Vec::new()),
                        |sd| {
                            (
                                sd.build_packages.to_selector_ids(),
                                sd.host_packages.to_selector_ids(),
                            )
                        },
                    );
                    Some(PackageData {
                        selector_id: SelectorId::new(p),
                        package: p,
                        build_packages,
                        host_packages,
                    })
                } else {
                    None
                }
            })
            .sorted()
            .collect();

        let platforms = {
            let mut tmp: Vec<_> = inner
                .platforms
                .iter()
                .map(SerializablePlatform::from_platform)
                .collect();
            tmp.sort_by_key(|p| p.name);
            tmp
        };

        let raw = SerializableLockFile::<V7> {
            version: FileFormatVersion::LATEST,
            platforms,
            environments,
            packages,
            _version: PhantomData::<V7>,
        };

        raw.serialize(serializer)
    }
}

#[derive(Debug, Clone)]
pub struct PackageData<'a> {
    pub selector_id: SelectorId,
    pub package: &'a LockedPackage,
    /// Pre-resolved selector ids for `source_data.build_packages`. Populated
    /// when building the serializable lockfile; empty for non-source packages.
    pub build_packages: Vec<SelectorId>,
    /// Pre-resolved selector ids for `source_data.host_packages`.
    pub host_packages: Vec<SelectorId>,
}

impl PartialEq for PackageData<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.package == other.package
    }
}

impl Eq for PackageData<'_> {}

impl PartialOrd<Self> for PackageData<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageData<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.selector_id.cmp(&other.selector_id)
    }
}

impl Serialize for CondaPackageData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            CondaPackageData::Binary(binary) => {
                SerializablePackageDataV7::Conda(v7::CondaPackageDataModel::from(binary.as_ref()))
                    .serialize(serializer)
            }
            CondaPackageData::Source(source) => {
                SerializablePackageDataV7::Source(v7::SourcePackageDataModel::from(source.as_ref()))
                    .serialize(serializer)
            }
        }
    }
}

impl Serialize for PypiPackageData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializablePackageDataV7::Pypi(v7::PypiPackageDataModel::from(self)).serialize(serializer)
    }
}
