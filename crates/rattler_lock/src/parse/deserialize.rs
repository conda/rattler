use crate::package::{CondaPackageData, PypiPackageData, RuntimePackageData};
use crate::{Channel, EnvironmentData, LockFile, ParseCondaLockError, PyPiRuntimeConfiguration};
use fxhash::FxHashMap;
use itertools::{Either, Itertools};
use rattler_conda_types::Platform;
use serde::Deserialize;
use serde_yaml::Value;
use std::collections::{BTreeMap, HashSet};
use url::Url;

#[derive(Deserialize)]
struct DeserializableLockFile {
    environments: BTreeMap<String, DeserializableEnvironment>,
    packages: Vec<DeserializablePackageData>,
}

#[derive(Deserialize)]
struct DeserializableEnvironment {
    channels: Vec<Channel>,
    packages: BTreeMap<Platform, Vec<DeserializablePackageSelector>>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DeserializablePackageData {
    Conda(Box<CondaPackageData>),
    Pypi(Box<PypiPackageData>),
}

#[derive(Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
enum DeserializablePackageSelector {
    Conda {
        conda: Url,
    },
    Pypi {
        pypi: Url,
        #[serde(default)]
        extras: HashSet<String>,
    },
}

/// Parses a [`LockFile`] from a [`serde_yaml::Value`].
pub fn parse_from_document(document: Value) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFile =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;

    // Split the packages into conda and pypi packages.
    let (conda_packages, pypi_packages): (Vec<_>, Vec<_>) =
        raw.packages.into_iter().partition_map(|p| match p {
            DeserializablePackageData::Conda(p) => Either::Left(*p),
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
        .map(|(idx, p)| (&p.url, idx))
        .collect::<FxHashMap<_, _>>();

    let environments = raw
        .environments
        .into_iter()
        .map(|(name, env)| {
            Ok((
                name.clone(),
                EnvironmentData {
                    channels: env.channels,
                    packages: env
                        .packages
                        .into_iter()
                        .map(|(platform, packages)| {
                            let packages = packages
                                .into_iter()
                                .map(|p| {
                                    Ok(match p {
                                        DeserializablePackageSelector::Conda { conda } => {
                                            RuntimePackageData::Conda(
                                                *conda_url_lookup.get(&conda).ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        name.clone(),
                                                        platform,
                                                        conda,
                                                    )
                                                })?,
                                            )
                                        }
                                        DeserializablePackageSelector::Pypi { pypi, extras } => {
                                            RuntimePackageData::Pypi(
                                                *pypi_url_lookup.get(&pypi).ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        name.clone(),
                                                        platform,
                                                        pypi,
                                                    )
                                                })?,
                                                PyPiRuntimeConfiguration { extras },
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
        .collect::<Result<_, ParseCondaLockError>>()?;

    Ok(LockFile {
        environments,
        conda_packages,
        pypi_packages,
    })
}
