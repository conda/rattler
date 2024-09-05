use crate::file_format_version::FileFormatVersion;
use crate::utils::serde::RawCondaPackageData;
use crate::{
    Channel, CondaPackageData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    ParseCondaLockError, PypiIndexes, PypiPackageData, PypiPackageEnvironmentData, UrlOrPath,
};
use fxhash::FxHashMap;
use indexmap::IndexSet;
use itertools::{Either, Itertools};
use pep508_rs::ExtraName;
use rattler_conda_types::Platform;
use serde::Deserialize;
use serde_yaml::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use url::Url;

#[derive(Deserialize)]
struct DeserializableLockFile<'d> {
    environments: BTreeMap<String, DeserializableEnvironment>,
    packages: Vec<DeserializablePackageData<'d>>,
}

#[derive(Deserialize)]
struct DeserializableEnvironment {
    channels: Vec<Channel>,
    #[serde(flatten)]
    indexes: Option<PypiIndexes>,
    packages: BTreeMap<Platform, Vec<DeserializablePackageSelector>>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DeserializablePackageData<'d> {
    Conda(Box<RawCondaPackageData<'d>>),
    Pypi(Box<PypiPackageData>),
}

#[derive(Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
enum DeserializablePackageSelector {
    Conda {
        conda: Url,
    },
    Pypi {
        pypi: UrlOrPath,
        #[serde(flatten)]
        runtime: DeserializablePypiPackageEnvironmentData,
    },
}

#[derive(Hash, Deserialize, Eq, PartialEq)]
struct DeserializablePypiPackageEnvironmentData {
    #[serde(default)]
    extras: BTreeSet<ExtraName>,
}

impl From<DeserializablePypiPackageEnvironmentData> for PypiPackageEnvironmentData {
    fn from(config: DeserializablePypiPackageEnvironmentData) -> Self {
        Self {
            extras: config.extras.into_iter().collect(),
        }
    }
}

/// Parses a [`LockFile`] from a [`serde_yaml::Value`].
pub fn parse_from_document(
    document: Value,
    version: FileFormatVersion,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFile<'_> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;

    // Split the packages into conda and pypi packages.
    let (conda_packages, pypi_packages): (Vec<_>, Vec<_>) =
        raw.packages.into_iter().partition_map(|p| match p {
            DeserializablePackageData::Conda(p) => Either::Left(CondaPackageData::from(*p)),
            DeserializablePackageData::Pypi(p) => Either::Right(*p),
        });

    // Determine the indices of the packages by url
    let conda_url_lookup = conda_packages
        .iter()
        .enumerate()
        .map(|(idx, p)| (&p.url, idx))
        .collect::<FxHashMap<_, _>>();
    let pypi_url_lookup = pypi_packages
        .iter()
        .enumerate()
        .map(|(idx, p)| (&p.url_or_path, idx))
        .collect::<FxHashMap<_, _>>();
    let mut pypi_runtime_lookup = IndexSet::new();

    let environments = raw
        .environments
        .into_iter()
        .map(|(name, env)| {
            Ok((
                name.clone(),
                EnvironmentData {
                    channels: env.channels,
                    indexes: env.indexes,
                    packages: env
                        .packages
                        .into_iter()
                        .map(|(platform, packages)| {
                            let packages = packages
                                .into_iter()
                                .map(|p| {
                                    Ok(match p {
                                        DeserializablePackageSelector::Conda { conda } => {
                                            EnvironmentPackageData::Conda(
                                                *conda_url_lookup.get(&conda).ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        name.clone(),
                                                        platform,
                                                        UrlOrPath::Url(conda),
                                                    )
                                                })?,
                                            )
                                        }
                                        DeserializablePackageSelector::Pypi { pypi, runtime } => {
                                            EnvironmentPackageData::Pypi(
                                                *pypi_url_lookup.get(&pypi).ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        name.clone(),
                                                        platform,
                                                        pypi,
                                                    )
                                                })?,
                                                pypi_runtime_lookup.insert_full(runtime).0,
                                            )
                                        }
                                    })
                                })
                                .collect::<Result<_, ParseCondaLockError>>()?;

                            Ok((platform, packages))
                        })
                        .collect::<Result<_, ParseCondaLockError>>()?,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, ParseCondaLockError>>()?;

    let (environment_lookup, environments) = environments
        .into_iter()
        .enumerate()
        .map(|(idx, (name, env))| ((name, idx), env))
        .unzip();

    Ok(LockFile {
        inner: Arc::new(LockFileInner {
            version,
            environments,
            environment_lookup,
            conda_packages,
            pypi_packages,
            pypi_environment_package_data: pypi_runtime_lookup
                .into_iter()
                .map(Into::into)
                .collect(),
        }),
    })
}
