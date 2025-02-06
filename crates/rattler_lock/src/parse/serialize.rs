use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashSet},
    marker::PhantomData,
};

use itertools::Itertools;
use pep508_rs::ExtraName;
use rattler_conda_types::{PackageName, Platform, VersionWithSource};
use serde::{Serialize, Serializer};
use serde_with::{serde_as, SerializeAs};
use url::Url;

use crate::{
    file_format_version::FileFormatVersion,
    parse::{models::v6, V6},
    Channel, CondaPackageData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    PypiIndexes, PypiPackageData, PypiPackageEnvironmentData, UrlOrPath,
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
    packages: BTreeMap<Platform, Vec<SerializablePackageSelector<'a>>>,
}

impl<'a> SerializableEnvironment<'a> {
    fn from_environment(
        inner: &'a LockFileInner,
        env_data: &'a EnvironmentData,
        used_conda_packages: &HashSet<usize>,
        used_pypi_packages: &HashSet<usize>,
    ) -> Self {
        SerializableEnvironment {
            channels: &env_data.channels,
            indexes: env_data.indexes.as_ref(),
            packages: env_data
                .packages
                .iter()
                .map(|(platform, packages)| {
                    (
                        *platform,
                        packages
                            .iter()
                            .map(|&package_data| {
                                SerializablePackageSelector::from_lock_file(
                                    inner,
                                    package_data,
                                    used_conda_packages,
                                    used_pypi_packages,
                                )
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
enum SerializablePackageDataV6<'a> {
    Conda(v6::CondaPackageDataModel<'a>),
    Pypi(v6::PypiPackageDataModel<'a>),
}

impl<'a> From<PackageData<'a>> for SerializablePackageDataV6<'a> {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<&'a VersionWithSource>,
        #[serde(skip_serializing_if = "Option::is_none")]
        build: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        subdir: Option<&'a str>,
    },
    Pypi {
        pypi: &'a UrlOrPath,
        #[serde(skip_serializing_if = "BTreeSet::is_empty")]
        extras: &'a BTreeSet<ExtraName>,
    },
}

#[derive(Copy, Clone)]
enum CondaDisambiguityFilter {
    Name,
    Version,
    Build,
    Subdir,
}

impl CondaDisambiguityFilter {
    fn all() -> [CondaDisambiguityFilter; 4] {
        [Self::Name, Self::Version, Self::Build, Self::Subdir]
    }

    fn filter(&self, package: &CondaPackageData, other: &CondaPackageData) -> bool {
        match self {
            Self::Name => package.record().name == other.record().name,
            Self::Version => package.record().version == other.record().version,
            Self::Build => package.record().build == other.record().build,
            Self::Subdir => package.record().subdir == other.record().subdir,
        }
    }
}

impl<'a> SerializablePackageSelector<'a> {
    fn from_lock_file(
        inner: &'a LockFileInner,
        package: EnvironmentPackageData,
        used_conda_packages: &HashSet<usize>,
        used_pypi_packages: &HashSet<usize>,
    ) -> Self {
        match package {
            EnvironmentPackageData::Conda(idx) => {
                Self::from_conda(inner, &inner.conda_packages[idx], used_conda_packages)
            }
            EnvironmentPackageData::Pypi(pkg_data_idx, env_data_idx) => Self::from_pypi(
                inner,
                &inner.pypi_packages[pkg_data_idx],
                &inner.pypi_environment_package_data[env_data_idx],
                used_pypi_packages,
            ),
        }
    }

    fn from_conda(
        inner: &'a LockFileInner,
        package: &'a CondaPackageData,
        used_conda_packages: &HashSet<usize>,
    ) -> Self {
        // Find all packages that share the same location
        let mut similar_packages = inner
            .conda_packages
            .iter()
            .enumerate()
            .filter_map(|(idx, p)| used_conda_packages.contains(&idx).then_some(p))
            .filter(|p| p.location() == package.location())
            .collect::<Vec<_>>();

        // Iterate over other distinguising factors and reduce the set of possible
        // packages to a minimum with the least number of keys added.
        let mut name = None;
        let mut version = None;
        let mut build = None;
        let mut subdir = None;
        while similar_packages.len() > 1 {
            let (filter, similar) = CondaDisambiguityFilter::all()
                .into_iter()
                .map(|filter| {
                    (
                        filter,
                        similar_packages
                            .iter()
                            .copied()
                            .filter(|p| filter.filter(package, p))
                            .collect_vec(),
                    )
                })
                .min_by_key(|(_filter, set)| set.len())
                .expect("cannot be empty because the set should always contain `package`");

            if similar.len() == similar_packages.len() {
                // No further disambiguation possible. Assume that the package is a duplicate.
                break;
            }

            similar_packages = similar;
            match filter {
                CondaDisambiguityFilter::Name => {
                    name = Some(&package.record().name);
                }
                CondaDisambiguityFilter::Version => {
                    version = Some(&package.record().version);
                }
                CondaDisambiguityFilter::Build => {
                    build = Some(package.record().build.as_str());
                }
                CondaDisambiguityFilter::Subdir => {
                    subdir = Some(package.record().subdir.as_str());
                }
            }
        }

        Self::Conda {
            conda: package.location(),
            name,
            version,
            build,
            subdir,
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
                    build: build_a,
                    version: version_a,
                    subdir: subdir_a,
                },
                SerializablePackageSelector::Conda {
                    conda: b,
                    name: name_b,
                    build: build_b,
                    version: version_b,
                    subdir: subdir_b,
                },
            ) => compare_url_by_location(a, b)
                .then_with(|| name_a.cmp(name_b))
                .then_with(|| version_a.cmp(version_b))
                .then_with(|| build_a.cmp(build_b))
                .then_with(|| subdir_a.cmp(subdir_b)),
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

impl<'a> SerializeAs<PackageData<'a>> for V6 {
    fn serialize_as<S>(source: &PackageData<'a>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializablePackageDataV6::from(*source).serialize(serializer)
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
        let environments = inner
            .environment_lookup
            .iter()
            .map(|(name, env_idx)| {
                (
                    name,
                    SerializableEnvironment::from_environment(
                        inner,
                        &inner.environments[*env_idx],
                        &used_conda_packages,
                        &used_pypi_packages,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();

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

        let raw = SerializableLockFile {
            version: FileFormatVersion::LATEST,
            environments,
            packages: packages.collect(),
            _version: PhantomData::<V6>,
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
        SerializablePackageDataV6::Conda(v6::CondaPackageDataModel::from(self))
            .serialize(serializer)
    }
}

impl Serialize for PypiPackageData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SerializablePackageDataV6::Pypi(v6::PypiPackageDataModel::from(self)).serialize(serializer)
    }
}
