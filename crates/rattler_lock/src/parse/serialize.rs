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
    parse::{models::v7, V7},
    Channel, CondaPackageData, EnvironmentData, LockFile, LockFileInner, LockedPackage,
    PackageIndex, PlatformData, PypiIndexes, PypiPackageData, SolveOptions,
};

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
                            .map(|&package_data| {
                                SerializablePackageSelector::from_lock_file(inner, package_data)
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
        match package {
            PackageData::Conda(CondaPackageData::Binary(binary)) => {
                Self::Conda(binary.as_ref().into())
            }
            PackageData::Conda(CondaPackageData::Source(source)) => {
                Self::Source(source.as_ref().into())
            }
            PackageData::Pypi(p) => Self::Pypi(p.into()),
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
        let id = pkg.selector_id();
        match pkg {
            crate::LockedPackage::Conda(CondaPackageData::Binary(_)) => Self::Conda { conda: id },
            crate::LockedPackage::Conda(CondaPackageData::Source(_)) => {
                Self::CondaSource { conda_source: id }
            }
            crate::LockedPackage::Pypi(_) => Self::Pypi { pypi: id },
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
        let mut used_packages = HashSet::new();
        for env in inner.environments.iter() {
            for packages in env.packages.values() {
                for package in packages {
                    used_packages.insert(*package);
                }
            }
        }

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
                match p {
                    // Deduplicate binary packages by location
                    LockedPackage::Conda(p) => match p {
                        CondaPackageData::Binary(binary) => seen_binary_locations
                            .insert(binary.location.clone())
                            .then_some(PackageData::Conda(p)),

                        CondaPackageData::Source(_) => Some(PackageData::Conda(p)),
                    },
                    LockedPackage::Pypi(p) => Some(PackageData::Pypi(p)),
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

        let raw = SerializableLockFile {
            version: FileFormatVersion::LATEST,
            platforms,
            environments,
            packages,
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
            PackageData::Pypi(p) => p.name().as_ref(),
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
