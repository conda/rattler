use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
    marker::PhantomData,
};

use itertools::Itertools;
use rattler_conda_types::Platform;
use serde::{Serialize, Serializer};
use serde_with::{serde_as, SerializeAs};
use url::Url;

use crate::{
    file_format_version::FileFormatVersion,
    parse::{models::v7, V7},
    Channel, CondaPackageData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    PypiIndexes, PypiPackageData, SolveOptions, SourceIdentifier, UrlOrPath,
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
    fn from_environment(inner: &'a LockFileInner, env_data: &'a EnvironmentData) -> Self {
        SerializableEnvironment {
            channels: &env_data.channels,
            indexes: env_data.indexes.as_ref(),
            options: env_data.options.clone(),
            packages: env_data
                .packages
                .iter()
                .map(|(platform, packages)| {
                    (
                        *platform,
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
            PackageData::Conda(CondaPackageData::Binary(binary)) => Self::Conda(binary.into()),
            PackageData::Conda(CondaPackageData::Source(source)) => Self::Source(source.into()),
            PackageData::Pypi(p) => Self::Pypi(p.into()),
        }
    }
}

/// Package selector for V7+ environments.
///
/// For V7+, binary conda packages are uniquely identified by their URL (which includes the
/// filename), and source packages use `SourceIdentifier` with an embedded hash.
#[derive(Serialize, Eq, PartialEq)]
#[serde(untagged, rename_all = "snake_case")]
enum SerializablePackageSelector<'a> {
    /// Binary conda packages are uniquely identified by their URL.
    Conda { conda: &'a UrlOrPath },
    /// Source packages use `SourceIdentifier` which uniquely identifies the package
    /// via the format `name[hash] @ location`. No additional disambiguation fields needed.
    Source { source: SourceIdentifier },
    /// Pypi packages are uniquely identified by their URL.
    Pypi { pypi: &'a UrlOrPath },
}

impl<'a> SerializablePackageSelector<'a> {
    fn from_lock_file(inner: &'a LockFileInner, package: EnvironmentPackageData) -> Self {
        match package {
            EnvironmentPackageData::Conda(idx) => Self::from_conda(&inner.conda_packages[idx]),
            EnvironmentPackageData::Pypi(pkg_data_idx) => {
                Self::from_pypi(&inner.pypi_packages[pkg_data_idx])
            }
        }
    }

    fn from_conda(package: &'a CondaPackageData) -> Self {
        match package {
            // Source packages use SourceIdentifier with an embedded hash
            CondaPackageData::Source(source_data) => Self::Source {
                source: SourceIdentifier::from_source_data(source_data),
            },
            // Binary packages are uniquely identified by their URL
            CondaPackageData::Binary(binary_data) => Self::Conda {
                conda: &binary_data.location,
            },
        }
    }

    fn from_pypi(package: &'a PypiPackageData) -> Self {
        Self::Pypi {
            pypi: &package.location,
        }
    }
}

impl PartialOrd for SerializablePackageSelector<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SerializablePackageSelector<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Helper to get package type ordering: Conda (0) < Source (1) < Pypi (2)
        fn type_order(selector: &SerializablePackageSelector<'_>) -> u8 {
            match selector {
                SerializablePackageSelector::Conda { .. } => 0,
                SerializablePackageSelector::Source { .. } => 1,
                SerializablePackageSelector::Pypi { .. } => 2,
            }
        }

        // First compare by type
        let type_cmp = type_order(self).cmp(&type_order(other));
        if type_cmp != Ordering::Equal {
            return type_cmp;
        }

        // Same type, compare by content
        match (self, other) {
            (
                SerializablePackageSelector::Source { source: a },
                SerializablePackageSelector::Source { source: b },
            ) => {
                // Compare by name first, then by hash, then by location
                a.name()
                    .cmp(b.name())
                    .then_with(|| a.hash().cmp(b.hash()))
                    .then_with(|| compare_url_by_location(a.location(), b.location()))
            }
            // Conda and Pypi both compare by location
            (
                SerializablePackageSelector::Conda { conda: a },
                SerializablePackageSelector::Conda { conda: b },
            )
            | (
                SerializablePackageSelector::Pypi { pypi: a, .. },
                SerializablePackageSelector::Pypi { pypi: b, .. },
            ) => compare_url_by_location(a, b),
            // Different types are already handled by type_cmp above
            _ => unreachable!(),
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
                        EnvironmentPackageData::Pypi(pkg_idx) => {
                            used_pypi_packages.insert(*pkg_idx);
                        }
                    }
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
                    SerializableEnvironment::from_environment(inner, &inner.environments[*env_idx]),
                )
            })
            .collect::<BTreeMap<_, _>>();

        // Get all packages, deduplicating binary packages by location.
        // V7 identifies binary packages by URL uniquely, so we deduplicate here
        // to handle older formats that may have had duplicate entries.
        // Source packages are NOT deduplicated because they use SourceIdentifier
        // which includes a hash to distinguish different configurations at the same location.
        let mut seen_binary_locations = HashSet::new();
        let conda_packages = inner
            .conda_packages
            .iter()
            .enumerate()
            .filter(|(idx, _)| used_conda_packages.contains(idx))
            .filter(|(_, p)| {
                match p {
                    // Deduplicate binary packages by location
                    CondaPackageData::Binary(binary) => {
                        seen_binary_locations.insert(binary.location.clone())
                    }
                    // Don't deduplicate source packages
                    CondaPackageData::Source(_) => true,
                }
            })
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
            PackageData::Conda(p) => p.record().name.as_source(),
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
        match self {
            CondaPackageData::Binary(binary) => {
                SerializablePackageDataV7::Conda(v7::CondaPackageDataModel::from(binary))
                    .serialize(serializer)
            }
            CondaPackageData::Source(source) => {
                SerializablePackageDataV7::Source(v7::SourcePackageDataModel::from(source))
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
