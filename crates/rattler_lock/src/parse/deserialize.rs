use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    path::Path,
    sync::Arc,
};

use indexmap::IndexSet;
use itertools::Either;
use pep440_rs::VersionSpecifiers;
use pep508_rs::ExtraName;
use rattler_conda_types::{PackageName, Platform, VersionWithSource};
use serde::{de::Error, Deserialize, Deserializer};
use serde_with::{serde_as, DeserializeAs};
use serde_yaml::Value;

use crate::{
    file_format_version::FileFormatVersion,
    parse::{
        models::{self, v6},
        V5, V6,
    },
    Channel, CondaPackageData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    PackageHashes, ParseCondaLockError, PypiIndexes, PypiPackageData, PypiPackageEnvironmentData,
    SolveOptions, UrlOrPath,
};

#[serde_as]
#[derive(Deserialize)]
#[serde(bound(deserialize = "P: DeserializeAs<'de, PackageData>"))]
struct DeserializableLockFile<P> {
    environments: BTreeMap<String, DeserializableEnvironment>,
    #[serde_as(as = "Vec<P>")]
    packages: Vec<PackageData>,
    #[serde(skip)]
    _data: PhantomData<P>,
}

/// A pinned Pypi package
#[derive(Eq, PartialEq, Clone, Debug, Hash)]
pub struct PypiPackageDataRaw {
    /// The name of the package.
    pub name: pep508_rs::PackageName,

    /// The version of the package.
    pub version: pep440_rs::Version,

    /// The location of the package. This can be a URL or a path.
    pub location: UrlOrPath,

    /// Hashes of the file pointed to by `url`.
    pub hash: Option<PackageHashes>,

    /// A list of (unparsed!) dependencies on other packages.
    pub requires_dist: Vec<String>,

    /// The python version that this package requires.
    pub requires_python: Option<VersionSpecifiers>,

    /// Whether the projects should be installed in editable mode or not.
    pub editable: bool,
}

impl From<PypiPackageData> for PypiPackageDataRaw {
    fn from(value: PypiPackageData) -> Self {
        let requires_dist = value
            .requires_dist
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>();

        Self {
            name: value.name.clone(),
            version: value.version.clone(),
            location: value.location.clone(),
            hash: value.hash.clone(),
            requires_dist,
            requires_python: value.requires_python.clone(),
            editable: value.editable,
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum PackageData {
    Conda(CondaPackageData),
    Pypi(PypiPackageDataRaw),
}

#[derive(Deserialize)]
struct DeserializableEnvironment {
    channels: Vec<Channel>,
    #[serde(flatten)]
    indexes: Option<PypiIndexes>,
    #[serde(default)]
    options: SolveOptions,
    packages: BTreeMap<Platform, Vec<DeserializablePackageSelector>>,
}

impl<'de> DeserializeAs<'de, PackageData> for V5 {
    fn deserialize_as<D>(deserializer: D) -> Result<PackageData, D::Error>
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
            Inner::Conda(c) => PackageData::Conda(c.into()),
            Inner::Pypi(p) => PackageData::Pypi(p.into()),
        })
    }
}

impl<'de> DeserializeAs<'de, PackageData> for V6 {
    fn deserialize_as<D>(deserializer: D) -> Result<PackageData, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Discriminant {
            Conda {
                #[serde(rename = "conda")]
                _conda: String,
            },
            Pypi {
                #[serde(rename = "pypi")]
                _pypi: String,
            },
        }

        let value = serde_value::Value::deserialize(deserializer)?;
        let Ok(discriminant) = Discriminant::deserialize(
            serde_value::ValueDeserializer::<D::Error>::new(value.clone()),
        ) else {
            return Err(D::Error::custom(
                "expected at least `conda` or `pypi` field",
            ));
        };

        let deserializer = serde_value::ValueDeserializer::<D::Error>::new(value);
        Ok(match discriminant {
            Discriminant::Conda { .. } => PackageData::Conda(
                v6::CondaPackageDataModel::deserialize(deserializer)?
                    .try_into()
                    .map_err(D::Error::custom)?,
            ),
            Discriminant::Pypi { .. } => {
                PackageData::Pypi(v6::PypiPackageDataModel::deserialize(deserializer)?.into())
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
enum DeserializablePackageSelector {
    Conda {
        conda: UrlOrPath,
        name: Option<PackageName>,
        version: Option<VersionWithSource>,
        build: Option<String>,
        subdir: Option<String>,
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
pub fn parse_from_document_v5(
    document: Value,
    version: FileFormatVersion,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFile<V5> =
        serde_yaml::from_value(document).map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock(version, raw, None)
}

pub fn parse_from_document_v6_and_v7(
    document: Value,
    version: FileFormatVersion,
    base_dir: Option<&Path>,
) -> Result<LockFile, ParseCondaLockError> {
    let raw: DeserializableLockFile<V6> =
        serde_yaml::from_value::<DeserializableLockFile<V6>>(document)
            .map_err(ParseCondaLockError::ParseError)?;
    parse_from_lock(version, raw, base_dir)
}

fn convert_raw_pypi_package(
    raw_package: PypiPackageDataRaw,
    base_dir: Option<&Path>,
) -> Result<PypiPackageData, ParseCondaLockError> {
    let requires_dist = raw_package
        .requires_dist
        .iter()
        .map(|input| {
            if let Some(base_dir) = base_dir {
                pep508_rs::Requirement::parse(input, base_dir)
            } else {
                use std::str::FromStr as _;

                pep508_rs::Requirement::from_str(input)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(PypiPackageData {
        name: raw_package.name,
        version: raw_package.version,
        location: raw_package.location,
        hash: raw_package.hash,
        requires_dist,
        requires_python: raw_package.requires_python,
        editable: raw_package.editable,
    })
}

fn parse_from_lock<P>(
    file_version: FileFormatVersion,
    raw: DeserializableLockFile<P>,
    base_dir: Option<&Path>,
) -> Result<LockFile, ParseCondaLockError> {
    // Split the packages into conda and pypi packages.
    let mut conda_packages = Vec::new();
    let mut pypi_packages = Vec::new();
    for package in raw.packages {
        match package {
            PackageData::Conda(p) => conda_packages.push(p),
            PackageData::Pypi(p) => {
                pypi_packages.push(convert_raw_pypi_package(p, base_dir)?);
            }
        }
    }

    // Determine the indices of the packages by url
    let mut conda_url_lookup: ahash::HashMap<UrlOrPath, Vec<_>> = ahash::HashMap::default();
    for (idx, conda_package) in conda_packages.iter().enumerate() {
        conda_url_lookup
            .entry(conda_package.location().clone())
            .or_default()
            .push(idx);
    }

    let pypi_url_lookup = pypi_packages
        .iter()
        .enumerate()
        .map(|(idx, p)| (&p.location, idx))
        .collect::<ahash::HashMap<_, _>>();
    let mut pypi_runtime_lookup = IndexSet::new();

    let environments = raw
        .environments
        .into_iter()
        .map(|(environment_name, env)| {
            Ok((
                environment_name.clone(),
                EnvironmentData {
                    channels: env.channels,
                    indexes: env.indexes,
                    options: env.options,
                    packages: env
                        .packages
                        .into_iter()
                        .map(|(platform, packages)| {
                            let packages = packages
                                .into_iter()
                                .map(|p| {
                                    Ok(match p {
                                        DeserializablePackageSelector::Conda {
                                            conda,
                                            name,
                                            version,
                                            build,
                                            subdir,
                                        } => {
                                            let all_packages = conda_url_lookup
                                                .get(&conda)
                                                .map_or(&[] as &[usize], Vec::as_slice);

                                            // Before V6 the package was selected by the first
                                            // match. This is actually a bug because when parsing an
                                            // older lock-file there can be more than one package
                                            // with the same url. Instead, we should look at the
                                            // subdir to disambiguate.
                                            let package_index: Option<usize> = if file_version
                                                < FileFormatVersion::V6
                                            {
                                                // Find the packages that match the subdir of
                                                // this environment
                                                let mut all_packages_with_subdir = all_packages
                                                    .iter()
                                                    .filter(|&idx| {
                                                        let conda_package = &conda_packages[*idx];
                                                        conda_package.record().subdir.as_str()
                                                            == platform.as_str()
                                                    })
                                                    .peekable();

                                                // If there are no packages for the subdir, use all
                                                // packages.
                                                let mut matching_packages =
                                                    if all_packages_with_subdir.peek().is_some() {
                                                        Either::Left(all_packages_with_subdir)
                                                    } else {
                                                        Either::Right(all_packages.iter())
                                                    };

                                                // Merge all matching packages.
                                                if let Some(&first_package_idx) =
                                                    matching_packages.next()
                                                {
                                                    let merged_package = Cow::Borrowed(
                                                        &conda_packages[first_package_idx],
                                                    );
                                                    let package = matching_packages.fold(
                                                        merged_package,
                                                        |acc, &next_package_idx| {
                                                            if let Cow::Owned(merged) = acc.merge(
                                                                &conda_packages[next_package_idx],
                                                            ) {
                                                                Cow::Owned(merged)
                                                            } else {
                                                                acc
                                                            }
                                                        },
                                                    );
                                                    Some(match package {
                                                        Cow::Borrowed(_) => first_package_idx,
                                                        Cow::Owned(package) => {
                                                            conda_packages.push(package);
                                                            conda_packages.len() - 1
                                                        }
                                                    })
                                                } else {
                                                    None
                                                }
                                            } else {
                                                // Find the package that matches all the fields from
                                                // the selector.
                                                all_packages
                                                    .iter()
                                                    .find(|&idx| {
                                                        let conda_package = &conda_packages[*idx];
                                                        name.as_ref().is_none_or(|name| {
                                                            name == &conda_package.record().name
                                                        }) && version.as_ref().is_none_or(
                                                            |version| {
                                                                version
                                                                    == &conda_package
                                                                        .record()
                                                                        .version
                                                            },
                                                        ) && build.as_ref().is_none_or(|build| {
                                                            build == &conda_package.record().build
                                                        }) && subdir.as_ref().is_none_or(|subdir| {
                                                            subdir == &conda_package.record().subdir
                                                        })
                                                    })
                                                    .copied()
                                            };

                                            EnvironmentPackageData::Conda(
                                                package_index.ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        environment_name.clone(),
                                                        platform,
                                                        conda,
                                                    )
                                                })?,
                                            )
                                        }
                                        DeserializablePackageSelector::Pypi { pypi, runtime } => {
                                            EnvironmentPackageData::Pypi(
                                                *pypi_url_lookup.get(&pypi).ok_or_else(|| {
                                                    ParseCondaLockError::MissingPackage(
                                                        environment_name.clone(),
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
            version: file_version,
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
