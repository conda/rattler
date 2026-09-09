use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

use pep508_rs::Requirement;
use rattler_conda_lock::{Document, Error, LockFile as CepLockFile, Manager, Package};
use rattler_conda_types::{PackageName, PackageRecord, Platform, VersionWithSource};
use rattler_digest::{Md5, Sha256, parse_digest_from_hex};

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

/// Imports selected categories without solving, downloading, or expanding variables.
///
/// Empty declared platforms remain present in every destination environment.
/// Errors refer to original CEP paths, including both sides of conflicting selections.
///
/// ```
/// # fn example(cep: &rattler_conda_lock::LockFile) -> Result<(), rattler_conda_lock::Error> {
/// let options = rattler_lock::conda_lock::ImportOptions::default();
/// let pixi = rattler_lock::conda_lock::import(cep, &options)?;
/// assert!(pixi.default_environment().is_some());
/// # Ok(())
/// # }
/// ```
pub fn import(lock_file: &CepLockFile, options: &ImportOptions) -> Result<LockFile, Error> {
    lock_file.validate()?;
    if options.environments.is_empty() {
        return Err(Error::new(
            "options.environments",
            "at least one environment selection is required",
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
        let path = format!("options.environments.{name}");
        if name.trim().is_empty() || selected.is_empty() {
            return Err(Error::new(
                path,
                "environment names and category selections must be nonempty",
            ));
        }
        for category in selected {
            if !categories.contains(category.as_str()) {
                return Err(Error::new(
                    &path,
                    format!("category '{category}' does not occur in the lock file"),
                ));
            }
        }
    }
    let platforms = lock_file
        .metadata
        .platforms
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let subdir = Platform::from_str(name).map_err(|error| {
                Error::new(format!("metadata.platforms[{index}]"), error.to_string())
            })?;
            Ok(PlatformData {
                name: (&subdir).into(),
                subdir,
                virtual_packages: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let mut builder = LockFile::builder()
        .with_platforms(platforms)
        .map_err(|error| Error::new("metadata.platforms", error.to_string()))?;
    // The builder merges packages by artifact identity. Reject divergent records
    // before registration so one environment cannot change another's semantics.
    let mut artifacts: BTreeMap<(Manager, String), (LockedPackage, usize)> = BTreeMap::new();
    for (environment, categories) in &options.environments {
        // Declared platforms stay in the environment even when the selected
        // categories contain no package for them.
        for (index, platform) in lock_file.metadata.platforms.iter().enumerate() {
            builder
                .add_environment_platform(environment, platform)
                .map_err(|error| {
                    Error::new(format!("metadata.platforms[{index}]"), error.to_string())
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
        let mut selected = BTreeMap::<(Manager, String, String), (LockedPackage, usize)>::new();
        for (index, package) in lock_file.package.iter().enumerate() {
            if !categories.contains(&package.category) {
                continue;
            }
            let converted = convert(package, index)?;
            let name = match &converted {
                LockedPackage::Conda(data) => data.name().as_normalized().to_owned(),
                LockedPackage::Pypi(data) => data.name().to_string(),
            };
            let key = (package.manager, package.platform.clone(), name);
            if let Some((original, original_index)) = selected.get(&key) {
                if original != &converted {
                    return Err(conflict(
                        index,
                        *original_index,
                        "selected categories contain incompatible packages with the same name",
                    ));
                }
                continue;
            }
            let artifact_key = (package.manager, package.url.clone());
            if let Some((original, original_index)) = artifacts.get(&artifact_key) {
                if original != &converted {
                    return Err(conflict(
                        index,
                        *original_index,
                        "the same artifact has conflicting package metadata",
                    ));
                }
            } else {
                artifacts.insert(artifact_key, (converted.clone(), index));
            }
            selected.insert(key, (converted.clone(), index));
            builder
                .add_package(environment, &package.platform, converted)
                .map_err(|error| Error::new(format!("package[{index}]"), error.to_string()))?;
        }
    }
    Ok(builder.finish())
}

/// Imports a parsed document and attaches its original source spans to errors.
///
/// ```
/// # fn example(yaml: &str) -> Result<(), rattler_conda_lock::Error> {
/// let document = rattler_conda_lock::Document::parse(yaml)?;
/// let pixi = rattler_lock::conda_lock::import_document(&document, &Default::default())?;
/// # Ok(())
/// # }
/// ```
pub fn import_document(document: &Document, options: &ImportOptions) -> Result<LockFile, Error> {
    import(document.lock_file(), options).map_err(|error| document.contextualize(error))
}

fn conflict(index: usize, original: usize, message: &str) -> Error {
    Error::new(format!("package[{index}]"), message)
        .with_related_path(format!("package[{original}]"), "original package selection")
}

fn convert(package: &Package, index: usize) -> Result<LockedPackage, Error> {
    let path = format!("package[{index}]");
    if package.source.is_some() {
        return Err(Error::new(
            format!("{path}.source"),
            "source builds cannot be imported as immutable artifacts",
        ));
    }
    let url = super::artifact_url(&package.url, &format!("{path}.url"))?;
    let md5 = package
        .hash
        .md5
        .as_deref()
        .map(|value| {
            parse_digest_from_hex::<Md5>(value)
                .ok_or_else(|| Error::new(format!("{path}.hash.md5"), "invalid MD5 digest"))
        })
        .transpose()?;
    let sha256 = package
        .hash
        .sha256
        .as_deref()
        .map(|value| {
            parse_digest_from_hex::<Sha256>(value)
                .ok_or_else(|| Error::new(format!("{path}.hash.sha256"), "invalid SHA256 digest"))
        })
        .transpose()?;
    match package.manager {
        Manager::Conda => {
            if url.as_str() != package.url {
                return Err(Error::new(
                    format!("{path}.url"),
                    "conda artifact URL cannot be preserved without URL normalization",
                ));
            }
            let location = UrlOrPath::Url(url);
            let derived = LocationDerivedFields::new(&location);
            let file_name = derived.identifier.ok_or_else(|| {
                Error::new(format!("{path}.url"), "expected a conda archive filename")
            })?;
            let build = package.build.clone().or(derived.build).ok_or_else(|| {
                Error::new(
                    format!("{path}.build"),
                    "build cannot be reconstructed from the artifact URL",
                )
            })?;
            let name = PackageName::from_str(&package.name)
                .map_err(|error| Error::new(format!("{path}.name"), error.to_string()))?;
            let version = VersionWithSource::from_str(&package.version)
                .map_err(|error| Error::new(format!("{path}.version"), error.to_string()))?;
            let mut record = PackageRecord::new(name, version, build);
            // Matches the historical importer: a build string without a trailing
            // number carries no build number, and CEP never stores one.
            record.build_number = derive_build_number_from_build(&record.build).unwrap_or(0);
            record.subdir = derived.subdir.unwrap_or_else(|| package.platform.clone());
            if record.subdir != "noarch" && record.subdir != package.platform {
                return Err(Error::new(
                    format!("{path}.url"),
                    "artifact subdir disagrees with the selected platform",
                )
                .with_related_path(format!("{path}.platform"), "selected platform"));
            }
            (record.arch, record.platform) = derive_arch_and_platform(&record.subdir);
            record.noarch = derive_noarch_type(&record.subdir, &record.build);
            record.md5 = md5;
            record.sha256 = sha256;
            let mut names = BTreeMap::new();
            for (name, value) in &package.dependencies {
                let dependency_path = format!("{path}.dependencies.{name}");
                let text = format!("{name} {value}");
                let (normalized, _) = super::dependencies::conda_spec(&text, &dependency_path)?;
                if let Some(original) = names.insert(normalized, dependency_path.clone()) {
                    return Err(Error::new(
                        &dependency_path,
                        "dependency names normalize to the same name",
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
                return Err(Error::new(
                    format!("{path}.build"),
                    "Python artifacts cannot carry a conda build string",
                ));
            }
            let mut requires_dist = Vec::with_capacity(package.dependencies.len());
            let mut names = BTreeMap::new();
            for (name, value) in &package.dependencies {
                let dependency_path = format!("{path}.dependencies.{name}");
                let constraint = python_constraint(value, &dependency_path)?;
                let requirement = Requirement::from_str(&format!("{name}{constraint}"))
                    .map_err(|error| Error::new(&dependency_path, error.to_string()))?;
                let (normalized, _) =
                    super::dependencies::python_spec(&requirement, &dependency_path)?;
                if let Some(original) = names.insert(normalized, dependency_path.clone()) {
                    return Err(Error::new(
                        &dependency_path,
                        "dependency names normalize to the same name",
                    )
                    .with_related_path(original, "original dependency"));
                }
                requires_dist.push(requirement);
            }
            let data = PypiDistributionData {
                name: package
                    .name
                    .parse()
                    .map_err(|error: pep508_rs::InvalidNameError| {
                        Error::new(format!("{path}.name"), error.to_string())
                    })?,
                version: package.version.parse().map_err(
                    |error: pep440_rs::VersionParseError| {
                        Error::new(format!("{path}.version"), error.to_string())
                    },
                )?,
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
fn python_constraint(value: &str, path: &str) -> Result<String, Error> {
    let value = value.trim();
    if value == "*" || value.is_empty() {
        return Ok(String::new());
    }
    if value.contains("||") {
        return Err(Error::new(
            path,
            "alternative version constraints cannot be represented in a Python requirement",
        ));
    }
    if pep440_rs::Version::from_str(value).is_ok() {
        return Ok(format!("=={value}"));
    }
    Ok(value.to_owned())
}
