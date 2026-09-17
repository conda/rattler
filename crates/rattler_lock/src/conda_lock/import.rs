use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

use pep508_rs::Requirement;
use rattler_conda_lock::{Document, LockFile as CepLockFile, Manager, NodePath, Package};
use rattler_conda_types::{PackageName, PackageRecord, Platform, VersionWithSource};
use rattler_digest::{Md5, Sha256, parse_digest_from_hex};

use super::dependencies::{conda_spec, python_spec};
use super::error::{ChecksumAlgorithm, CondaLockError, CondaLockErrorKind};
use crate::utils::derived_fields::{
    LocationDerivedFields, derive_arch_and_platform, derive_build_number_from_build,
    derive_noarch_type,
};
use crate::{
    CondaBinaryData, LockFile, LockedPackage, PackageHashes, PlatformData, PypiDistributionData,
    UrlOrPath, Verbatim,
};

/// Exact category selections for each destination pixi environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportOptions {
    /// Environment names mapped to category sets. No implicit `main` category is added.
    pub environments: BTreeMap<String, BTreeSet<String>>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            environments: BTreeMap::from([("default".into(), BTreeSet::from(["main".into()]))]),
        }
    }
}

impl LockFile {
    /// Imports selected categories of a CEP-37 lock file, without solving,
    /// downloading, or expanding variables.
    ///
    /// Empty declared platforms remain present in every destination environment.
    /// Errors refer to CEP-37 model paths, including both sides of conflicting
    /// selections; pass a [`Document`] to [`Self::from_conda_lock_document`] to
    /// have those paths resolved to source spans as well.
    ///
    /// ```
    /// # fn example(cep: &rattler_conda_lock::LockFile) -> Result<(), rattler_lock::conda_lock::CondaLockError> {
    /// use rattler_lock::{LockFile, conda_lock::ImportOptions};
    ///
    /// let pixi = LockFile::from_conda_lock(cep, &ImportOptions::default())?;
    /// assert!(pixi.default_environment().is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_conda_lock(
        lock_file: &CepLockFile,
        options: &ImportOptions,
    ) -> Result<Self, CondaLockError> {
        import(lock_file, options)
    }

    /// Imports a parsed document, attaching its source spans to any error.
    ///
    /// ```
    /// # fn example(yaml: &str) -> Result<(), rattler_lock::conda_lock::CondaLockError> {
    /// use rattler_lock::{LockFile, conda_lock::ImportOptions};
    ///
    /// let document = rattler_conda_lock::Document::parse(yaml)?;
    /// let pixi = LockFile::from_conda_lock_document(&document, &ImportOptions::default())?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_conda_lock_document(
        document: &Document,
        options: &ImportOptions,
    ) -> Result<Self, CondaLockError> {
        import(document.lock_file(), options).map_err(|mut error| {
            document.locate(error.labels_mut());
            error
        })
    }
}

fn import(lock_file: &CepLockFile, options: &ImportOptions) -> Result<LockFile, CondaLockError> {
    lock_file.validate()?;
    let options_path = NodePath::root().field("options").field("environments");
    if options.environments.is_empty() {
        return Err(CondaLockError::new(
            options_path,
            CondaLockErrorKind::NoEnvironments,
        ));
    }
    let mut categories: BTreeSet<&str> = lock_file
        .package
        .iter()
        .map(|package| package.category.as_str())
        .collect();
    // An empty lock still has a meaningful empty main environment.
    categories.insert("main");
    for (name, selected) in &options.environments {
        let path = options_path.field(name);
        if name.trim().is_empty() || selected.is_empty() {
            return Err(CondaLockError::new(
                path,
                CondaLockErrorKind::EmptySelection,
            ));
        }
        for category in selected {
            if !categories.contains(category.as_str()) {
                return Err(CondaLockError::new(
                    path.clone(),
                    CondaLockErrorKind::UnknownCategory {
                        category: category.clone(),
                    },
                ));
            }
        }
    }
    let platforms_path = NodePath::root().field("metadata").field("platforms");
    let platforms = lock_file
        .metadata
        .platforms
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let subdir = Platform::from_str(name).map_err(|error| {
                CondaLockError::new(
                    platforms_path.index(index),
                    CondaLockErrorKind::UnsupportedPlatform(error),
                )
            })?;
            Ok(PlatformData {
                name: (&subdir).into(),
                subdir,
                virtual_packages: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, CondaLockError>>()?;
    let mut builder = LockFile::builder()
        .with_platforms(platforms)
        .map_err(|error| {
            CondaLockError::new(
                platforms_path.clone(),
                CondaLockErrorKind::Builder(Box::new(error)),
            )
        })?;
    let packages_path = NodePath::root().field("package");
    // The builder merges packages by artifact identity. Reject divergent records
    // before registration so one environment cannot change another's semantics.
    let mut artifacts: BTreeMap<ArtifactKey<'_>, (LockedPackage, usize)> = BTreeMap::new();
    for (environment, categories) in &options.environments {
        // Declared platforms stay in the environment even when the selected
        // categories contain no package for them.
        for (index, platform) in lock_file.metadata.platforms.iter().enumerate() {
            builder
                .add_environment_platform(environment, platform)
                .map_err(|error| {
                    CondaLockError::new(
                        platforms_path.index(index),
                        CondaLockErrorKind::Builder(Box::new(error)),
                    )
                })?;
        }
        builder.set_channels(
            environment,
            lock_file
                .metadata
                .channels
                .iter()
                .map(|channel| crate::Channel {
                    url: channel.url.clone(),
                    used_env_vars: channel.used_env_vars.clone(),
                }),
        );
        let mut selected = BTreeMap::<EnvironmentSlot<'_>, (LockedPackage, usize)>::new();
        for (index, package) in lock_file.package.iter().enumerate() {
            if !categories.contains(&package.category) {
                continue;
            }
            let path = packages_path.index(index);
            let converted = convert(package, &path)?;
            // One destination environment holds one package per normalized
            // name, whatever categories selected it.
            let slot = EnvironmentSlot::new(package, &converted);
            if let Some((original, original_index)) = selected.get(&slot) {
                if original != &converted {
                    return Err(conflict(
                        &packages_path,
                        index,
                        *original_index,
                        CondaLockErrorKind::IncompatibleSelection,
                    ));
                }
                continue;
            }
            let artifact = ArtifactKey {
                manager: package.manager,
                url: package.url.as_str(),
            };
            if let Some((original, original_index)) = artifacts.get(&artifact) {
                if original != &converted {
                    return Err(conflict(
                        &packages_path,
                        index,
                        *original_index,
                        CondaLockErrorKind::ConflictingArtifact,
                    ));
                }
            } else {
                artifacts.insert(artifact, (converted.clone(), index));
            }
            selected.insert(slot, (converted.clone(), index));
            builder
                .add_package(environment, &package.platform, converted)
                .map_err(|error| {
                    CondaLockError::new(path.clone(), CondaLockErrorKind::Builder(Box::new(error)))
                })?;
        }
    }
    Ok(builder.finish())
}

/// The slot a package occupies in one destination environment. A pixi
/// environment holds one package per manager, platform and normalized name,
/// however many CEP-37 categories selected it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EnvironmentSlot<'a> {
    manager: Manager,
    platform: &'a str,
    name: String,
}

impl<'a> EnvironmentSlot<'a> {
    fn new(package: &'a Package, converted: &LockedPackage) -> Self {
        Self {
            manager: package.manager,
            platform: &package.platform,
            name: match converted {
                LockedPackage::Conda(data) => data.name().as_normalized().to_owned(),
                LockedPackage::Pypi(data) => data.name().to_string(),
            },
        }
    }
}

/// The artifact a package resolves to. The pixi lock file stores one record per
/// artifact, so two CEP-37 packages sharing a URL must agree on its metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ArtifactKey<'a> {
    manager: Manager,
    url: &'a str,
}

fn conflict(
    packages: &NodePath,
    index: usize,
    original: usize,
    kind: CondaLockErrorKind,
) -> CondaLockError {
    CondaLockError::new(packages.index(index), kind)
        .with_related_path(packages.index(original), "original package selection")
}

fn convert(package: &Package, path: &NodePath) -> Result<LockedPackage, CondaLockError> {
    if package.source.is_some() {
        return Err(CondaLockError::new(
            path.field("source"),
            CondaLockErrorKind::SourcePackage,
        ));
    }
    let url_path = path.field("url");
    let url = super::artifact_url(&package.url, &url_path)?;
    let hash_path = path.field("hash");
    let md5 = package
        .hash
        .md5
        .as_deref()
        .map(|value| {
            parse_digest_from_hex::<Md5>(value).ok_or_else(|| {
                CondaLockError::new(
                    hash_path.field("md5"),
                    CondaLockErrorKind::InvalidDigest {
                        algorithm: ChecksumAlgorithm::Md5,
                    },
                )
            })
        })
        .transpose()?;
    let sha256 = package
        .hash
        .sha256
        .as_deref()
        .map(|value| {
            parse_digest_from_hex::<Sha256>(value).ok_or_else(|| {
                CondaLockError::new(
                    hash_path.field("sha256"),
                    CondaLockErrorKind::InvalidDigest {
                        algorithm: ChecksumAlgorithm::Sha256,
                    },
                )
            })
        })
        .transpose()?;
    let dependencies_path = path.field("dependencies");
    match package.manager {
        Manager::Conda => {
            if url.as_str() != package.url {
                return Err(CondaLockError::new(
                    url_path,
                    CondaLockErrorKind::UnnormalizedUrl,
                ));
            }
            let location = UrlOrPath::Url(url);
            let derived = LocationDerivedFields::new(&location);
            let file_name = derived.identifier.ok_or_else(|| {
                CondaLockError::new(url_path.clone(), CondaLockErrorKind::MissingFileName)
            })?;
            let build = package.build.clone().or(derived.build).ok_or_else(|| {
                CondaLockError::new(path.field("build"), CondaLockErrorKind::MissingBuildString)
            })?;
            let name = PackageName::from_str(&package.name).map_err(|error| {
                CondaLockError::new(
                    path.field("name"),
                    CondaLockErrorKind::InvalidCondaName(error),
                )
            })?;
            let version = VersionWithSource::from_str(&package.version).map_err(|error| {
                CondaLockError::new(
                    path.field("version"),
                    CondaLockErrorKind::InvalidCondaVersion(error),
                )
            })?;
            let mut record = PackageRecord::new(name, version, build);
            // Matches the historical importer: a build string without a trailing
            // number carries no build number, and CEP-37 never stores one.
            record.build_number = derive_build_number_from_build(&record.build).unwrap_or(0);
            record.subdir = derived.subdir.unwrap_or_else(|| package.platform.clone());
            if record.subdir != "noarch" && record.subdir != package.platform {
                return Err(
                    CondaLockError::new(url_path, CondaLockErrorKind::SubdirMismatch)
                        .with_related_path(path.field("platform"), "selected platform"),
                );
            }
            (record.arch, record.platform) = derive_arch_and_platform(&record.subdir);
            record.noarch = derive_noarch_type(&record.subdir, &record.build);
            record.md5 = md5;
            record.sha256 = sha256;
            let mut normalized = BTreeMap::new();
            for (name, value) in &package.dependencies {
                let dependency_path = dependencies_path.field(name);
                let text = format!("{name} {value}");
                let dependency = conda_spec(&text, &dependency_path)?;
                if let Some(original) = normalized.insert(dependency.name, dependency_path.clone())
                {
                    return Err(CondaLockError::new(
                        dependency_path,
                        CondaLockErrorKind::AmbiguousDependencyName,
                    )
                    .with_related_path(original, "original dependency"));
                }
                record.depends.push(text);
            }
            Ok(LockedPackage::Conda(
                CondaBinaryData {
                    package_record: record,
                    location,
                    file_name,
                    channel: derived.channel,
                }
                .into(),
            ))
        }
        Manager::Pip => {
            if package.build.is_some() {
                return Err(CondaLockError::new(
                    path.field("build"),
                    CondaLockErrorKind::PythonBuildString,
                ));
            }
            let mut requires_dist = Vec::with_capacity(package.dependencies.len());
            let mut normalized = BTreeMap::new();
            for (name, value) in &package.dependencies {
                let dependency_path = dependencies_path.field(name);
                let constraint = python_constraint(value, &dependency_path)?;
                let requirement =
                    Requirement::from_str(&format!("{name}{constraint}")).map_err(|error| {
                        CondaLockError::new(
                            dependency_path.clone(),
                            CondaLockErrorKind::InvalidRequirement(Box::new(error)),
                        )
                    })?;
                let dependency = python_spec(&requirement, &dependency_path)?;
                if let Some(original) = normalized.insert(dependency.name, dependency_path.clone())
                {
                    return Err(CondaLockError::new(
                        dependency_path,
                        CondaLockErrorKind::AmbiguousDependencyName,
                    )
                    .with_related_path(original, "original dependency"));
                }
                requires_dist.push(requirement);
            }
            let data = PypiDistributionData {
                name: package.name.parse().map_err(|error| {
                    CondaLockError::new(
                        path.field("name"),
                        CondaLockErrorKind::InvalidPythonName(error),
                    )
                })?,
                version: package.version.parse().map_err(|error| {
                    CondaLockError::new(
                        path.field("version"),
                        CondaLockErrorKind::InvalidPythonVersion(error),
                    )
                })?,
                location: Verbatim::new_with_given(UrlOrPath::Url(url), package.url.clone()),
                index_url: None,
                hash: PackageHashes::from_hashes(md5, sha256),
                requires_dist,
                requires_python: None,
            };
            Ok(LockedPackage::Pypi(data.into()))
        }
    }
}

/// Translates the constraint spellings conda-lock writes into a PEP 508 suffix.
///
/// `*` is unconstrained and a bare version literal is an exact pin. Alternatives
/// separated by `||` have no PEP 508 equivalent and are rejected.
fn python_constraint(value: &str, path: &NodePath) -> Result<String, CondaLockError> {
    let value = value.trim();
    if value == "*" || value.is_empty() {
        return Ok(String::new());
    }
    if value.contains("||") {
        return Err(CondaLockError::new(
            path.clone(),
            CondaLockErrorKind::AlternativeConstraints,
        ));
    }
    if pep440_rs::Version::from_str(value).is_ok() {
        return Ok(format!("=={value}"));
    }
    Ok(value.to_owned())
}
