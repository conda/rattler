use crate::{
    package::{CondaPackageData, PypiPackageData},
    parse::FILE_VERSION,
    Channel, LockFile, Package,
};
use itertools::Itertools;
use rattler_conda_types::Platform;
use serde::{Serialize, Serializer};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
};
use url::Url;

#[derive(Serialize)]
struct SerializableLockFile<'a> {
    version: u64,
    environments: BTreeMap<String, SerializableEnvironment<'a>>,
    packages: Vec<SerializablePackageData<'a>>,
}

#[derive(Serialize)]
struct SerializableEnvironment<'a> {
    channels: &'a [Channel],
    packages: BTreeMap<Platform, Vec<SerializablePackageSelector<'a>>>,
}

#[derive(Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SerializablePackageData<'a> {
    Conda(&'a CondaPackageData),
    Pypi(&'a PypiPackageData),
}

#[derive(Serialize)]
#[serde(untagged, rename_all = "snake_case")]
enum SerializablePackageSelector<'a> {
    Conda {
        conda: &'a Url,
    },
    Pypi {
        pypi: &'a Url,
        #[serde(skip_serializing_if = "HashSet::is_empty")]
        extras: &'a HashSet<String>,
    },
}

impl<'l> From<Package<'l>> for SerializablePackageSelector<'l> {
    fn from(value: Package<'l>) -> Self {
        match value {
            Package::Conda(p) => SerializablePackageSelector::Conda {
                conda: &p.package.url,
            },
            Package::Pypi(p) => SerializablePackageSelector::Pypi {
                pypi: &p.package.url,
                extras: &p.runtime.extras,
            },
        }
    }
}

impl<'a> SerializablePackageSelector<'a> {
    fn url(&self) -> &Url {
        match self {
            SerializablePackageSelector::Conda { conda } => conda,
            SerializablePackageSelector::Pypi { pypi, .. } => pypi,
        }
    }
}

impl<'a> SerializablePackageData<'a> {
    fn name(&self) -> &'a str {
        match self {
            SerializablePackageData::Conda(p) => p.package_record.name.as_normalized(),
            SerializablePackageData::Pypi(p) => &p.name,
        }
    }

    fn url(&self) -> &'a Url {
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
        // Get all packages.
        let mut packages = self
            .conda_packages
            .iter()
            .map(SerializablePackageData::Conda)
            .chain(self.pypi_packages.iter().map(SerializablePackageData::Pypi))
            .collect::<Vec<_>>();

        // Get all environments
        let environments = self
            .environments()
            .map(|(name, env)| {
                (
                    name.to_string(),
                    SerializableEnvironment {
                        channels: env.channels(),
                        packages: env
                            .platforms()
                            .filter_map(|platform| {
                                let packages = env.packages(platform)?;
                                Some((
                                    platform,
                                    packages
                                        .sorted_by_key(Package::url)
                                        .map(Into::into)
                                        .collect(),
                                ))
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
