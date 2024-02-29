use super::FILE_VERSION;
use crate::utils::serde::RawCondaPackageData;
use crate::{Channel, EnvironmentPackageData, LockFile, PypiPackageData};
use itertools::Itertools;
use rattler_conda_types::Platform;
use serde::{Serialize, Serializer};
use std::collections::{BTreeSet, HashSet};
use std::{cmp::Ordering, collections::BTreeMap};
use url::Url;

#[derive(Serialize)]
struct SerializableLockFile<'a> {
    version: u64,
    environments: BTreeMap<&'a String, SerializableEnvironment<'a>>,
    packages: Vec<SerializablePackageData<'a>>,
}

#[derive(Serialize)]
struct SerializableEnvironment<'a> {
    channels: &'a [Channel],
    packages: BTreeMap<Platform, Vec<SerializablePackageSelector<'a>>>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SerializablePackageData<'a> {
    Conda(RawCondaPackageData<'a>),
    Pypi(&'a PypiPackageData),
}

#[derive(Serialize, Eq, PartialEq)]
#[serde(untagged, rename_all = "snake_case")]
enum SerializablePackageSelector<'a> {
    Conda {
        conda: &'a Url,
    },
    Pypi {
        pypi: &'a Url,
        #[serde(skip_serializing_if = "BTreeSet::is_empty")]
        extras: &'a BTreeSet<String>,
    },
}

impl<'a> SerializablePackageSelector<'a> {
    fn url(&self) -> &Url {
        match self {
            SerializablePackageSelector::Conda { conda } => conda,
            SerializablePackageSelector::Pypi { pypi, .. } => pypi,
        }
    }
}

impl<'a> PartialOrd for SerializablePackageSelector<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for SerializablePackageSelector<'a> {
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
                SerializablePackageSelector::Conda { conda: a },
                SerializablePackageSelector::Conda { conda: b },
            )
            | (
                SerializablePackageSelector::Pypi { pypi: a, .. },
                SerializablePackageSelector::Pypi { pypi: b, .. },
            ) => {
                // First sort packages just by their filename. Since most of the time the urls end
                // in the packages filename this causes the urls to be sorted by package name.
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
        }
    }
}

impl<'a> SerializablePackageData<'a> {
    fn name(&self) -> &str {
        match self {
            SerializablePackageData::Conda(p) => p.name.as_normalized(),
            SerializablePackageData::Pypi(p) => p.name.as_ref(),
        }
    }

    fn url(&self) -> &Url {
        match self {
            SerializablePackageData::Conda(p) => &p.url,
            SerializablePackageData::Pypi(p) => &p.url,
        }
    }
}

impl PartialOrd for SerializablePackageData<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SerializablePackageData<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        use SerializablePackageData::{Conda, Pypi};
        // First sort by name, then by package type specific attributes
        self.name()
            .cmp(other.name())
            .then_with(|| match (self, other) {
                (Conda(a), Conda(b)) => a.cmp(b),
                (Pypi(a), Pypi(b)) => a.cmp(b),
                (Pypi(_), _) => Ordering::Less,
                (_, Pypi(_)) => Ordering::Greater,
            })
    }
}

impl Serialize for LockFile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let inner = self.inner.as_ref();

        // Get all packages.
        let mut packages = inner
            .conda_packages
            .iter()
            .map(RawCondaPackageData::from)
            .map(SerializablePackageData::Conda)
            .chain(
                inner
                    .pypi_packages
                    .iter()
                    .map(SerializablePackageData::Pypi),
            )
            .collect::<Vec<_>>();

        // Get all environments
        let environments = inner
            .environment_lookup
            .iter()
            .map(|(name, env_idx)| {
                let env_data = &inner.environments[*env_idx];
                (
                    name,
                    SerializableEnvironment {
                        channels: &env_data.channels,
                        packages: env_data
                            .packages
                            .iter()
                            .map(|(platform, packages)| {
                                (
                                    *platform,
                                    packages
                                        .iter()
                                        .map(|package_data| match *package_data {
                                            EnvironmentPackageData::Conda(conda_index) => {
                                                SerializablePackageSelector::Conda {
                                                    conda: &inner.conda_packages[conda_index].url,
                                                }
                                            }
                                            EnvironmentPackageData::Pypi(
                                                pypi_index,
                                                pypi_runtime_index,
                                            ) => {
                                                let pypi_package = &inner.pypi_packages[pypi_index];
                                                let pypi_runtime = &inner
                                                    .pypi_environment_package_datas
                                                    [pypi_runtime_index];
                                                SerializablePackageSelector::Pypi {
                                                    pypi: &pypi_package.url,
                                                    extras: &pypi_runtime.extras,
                                                }
                                            }
                                        })
                                        .sorted()
                                        .collect(),
                                )
                            })
                            .collect(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        // Determine the URLs that are used in the environments.
        let used_urls_in_envs = environments
            .values()
            .flat_map(|env| {
                env.packages
                    .values()
                    .flat_map(|packages| packages.iter().map(SerializablePackageSelector::url))
            })
            .collect::<HashSet<_>>();

        // Only retain the packages that are used in the environments.
        packages.retain(|p| used_urls_in_envs.contains(&p.url()));

        // Sort the packages in a deterministic order. See [`SerializablePackageData`] for more
        // information.
        packages.sort();

        let raw = SerializableLockFile {
            version: FILE_VERSION,
            environments,
            packages,
        };

        raw.serialize(serializer)
    }
}
