use std::collections::BTreeMap;

use rattler_conda_lock::{Channel, Error, Hashes, LockFile, Manager, Metadata, Package};

use crate::utils::derived_fields::{LocationDerivedFields, derive_noarch_type};
use crate::{CondaPackageData, Environment, LockedPackage, PypiPackageData, UrlOrPath};

/// Metadata supplied by the caller when exporting a single environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Original specification filenames. Defaults to an empty required CEP list.
    pub sources: Vec<String>,
    /// Exact per-platform content hashes. When absent, canonical package hashes
    /// are computed; these are not conda-lock's original input-specification hashes.
    pub content_hash: Option<BTreeMap<String, String>>,
}

/// Exports one environment as nonoptional `main` CEP packages, entirely offline.
///
/// Solver-only repodata and provenance may be omitted. Source builds and install
/// semantics that CEP cannot represent are errors. Channel order is preserved.
///
/// ```
/// # fn example(environment: rattler_lock::Environment<'_>) -> Result<(), rattler_conda_lock::Error> {
/// let options = rattler_lock::conda_lock::ExportOptions {
///     sources: vec!["environment.yml".into()],
///     ..Default::default()
/// };
/// let cep = rattler_lock::conda_lock::export(environment, &options)?;
/// let yaml = cep.to_yaml()?;
/// # Ok(())
/// # }
/// ```
pub fn export(environment: Environment<'_>, options: &ExportOptions) -> Result<LockFile, Error> {
    let mut result = LockFile {
        metadata: Metadata {
            channels: environment
                .channels()
                .iter()
                .map(|channel| Channel {
                    url: channel.url.clone(),
                    used_env_vars: channel.used_env_vars.clone(),
                })
                .collect(),
            sources: options.sources.clone(),
            ..Metadata::default()
        },
        package: Vec::new(),
    };
    let mut platforms: Vec<_> = environment.platforms().collect();
    platforms.sort_by(|left, right| left.name().as_str().cmp(right.name().as_str()));
    let mut subdirs = BTreeMap::new();
    for (platform_index, platform) in platforms.into_iter().enumerate() {
        let platform_path = format!("metadata.platforms[{platform_index}]");
        let subdir = platform.subdir().to_string();
        if let Some(original) = subdirs.insert(subdir.clone(), platform_path.clone()) {
            return Err(Error::new(
                &platform_path,
                "multiple pixi platforms collapse to the same CEP subdir",
            )
            .with_related_path(original, "first platform with this subdir"));
        }
        if platform.name().as_str() != subdir {
            return Err(Error::new(
                &platform_path,
                "custom platform names cannot be preserved in CEP",
            ));
        }
        if !platform.virtual_packages().is_empty() {
            return Err(Error::new(
                &platform_path,
                "custom virtual-package requirements cannot be represented in CEP",
            ));
        }
        result.metadata.platforms.push(subdir.clone());
        let mut names = BTreeMap::new();
        for locked in environment.packages(platform).into_iter().flatten() {
            let path = format!("package[{}]", result.package.len());
            let package = convert(locked, &subdir, &path)?;
            let normalized = match locked {
                LockedPackage::Conda(data) => data.name().as_normalized().to_owned(),
                LockedPackage::Pypi(_) => package.name.clone(),
            };
            if let Some(original) = names.insert((package.manager, normalized), path.clone()) {
                return Err(Error::new(&path, "multiple artifacts with the same manager and name cannot be represented in one CEP category")
                    .with_related_path(original, "first package with this name"));
            }
            result.package.push(package);
        }
    }
    result.metadata.content_hash = options
        .content_hash
        .clone()
        .unwrap_or_else(|| result.compute_content_hashes());
    result.validate()?;
    Ok(result)
}

fn convert(locked: &LockedPackage, platform: &str, path: &str) -> Result<Package, Error> {
    let mut package = Package {
        name: String::new(),
        version: String::new(),
        manager: Manager::Conda,
        platform: platform.into(),
        dependencies: BTreeMap::new(),
        url: String::new(),
        hash: Hashes::default(),
        source: None,
        build: None,
        category: "main".into(),
        optional: false,
    };
    let mut original_dependencies = BTreeMap::new();
    match locked {
        LockedPackage::Conda(CondaPackageData::Source(_)) => {
            return Err(Error::new(
                path,
                "conda source builds cannot be exported as CEP artifacts",
            ));
        }
        LockedPackage::Conda(CondaPackageData::Binary(data)) => {
            let record = &data.package_record;
            let UrlOrPath::Url(url) = &data.location else {
                return Err(Error::new(
                    format!("{path}.url"),
                    "local conda artifact paths cannot be exported as URLs",
                ));
            };
            super::artifact_url(url.as_str(), &format!("{path}.url"))?;
            let derived = LocationDerivedFields::new(&data.location);
            if derived.identifier.as_ref() != Some(&data.file_name) {
                return Err(Error::new(
                    format!("{path}.url"),
                    "artifact filename cannot be reconstructed from its URL",
                ));
            }
            // CEP stores no subdir, so a reader derives it from the URL. Only a
            // record naming a *different* target would be silently relabelled;
            // the target platform itself is an accepted spelling for noarch.
            let reconstructed_subdir = derived.subdir.as_deref().unwrap_or(platform);
            if reconstructed_subdir != record.subdir && record.subdir != platform {
                return Err(Error::new(
                    format!("{path}.platform"),
                    "artifact subdir disagrees with both its URL and the selected platform",
                ));
            }
            if record.noarch != derive_noarch_type(reconstructed_subdir, &record.build) {
                return Err(Error::new(
                    format!("{path}.url"),
                    "noarch installation mode cannot be reconstructed from the artifact URL and build",
                ));
            }
            if record.python_site_packages_path.is_some() {
                return Err(Error::new(
                    path,
                    "custom Python site-packages installation paths cannot be represented in CEP",
                ));
            }
            if !record.extra_depends.is_empty() {
                return Err(Error::new(
                    format!("{path}.dependencies"),
                    "conditional or extra conda dependencies cannot be represented in CEP",
                ));
            }
            // Constraints, features, run exports, flags, timestamps, size, purls,
            // and licenses are solver/repodata information, not artifact identity.
            package.name = record.name.as_source().to_owned();
            package.version = record.version.as_str().into_owned();
            package.build = Some(record.build.clone());
            package.url = url.as_str().to_owned();
            package.hash.md5 = record.md5.map(hex::encode);
            package.hash.sha256 = record.sha256.map(hex::encode);
            for (index, dependency) in record.depends.iter().enumerate() {
                let dependency_path = format!("{path}.dependencies[{index}]");
                let (name, value) = super::dependencies::conda_spec(dependency, &dependency_path)?;
                super::dependencies::insert(
                    &mut package.dependencies,
                    &mut original_dependencies,
                    name,
                    value,
                    dependency_path,
                )?;
            }
        }
        LockedPackage::Pypi(PypiPackageData::Source(_)) => {
            return Err(Error::new(
                path,
                "local Python source trees cannot be exported as CEP artifacts",
            ));
        }
        LockedPackage::Pypi(PypiPackageData::Distribution(data)) => {
            let UrlOrPath::Url(url) = data.location.inner() else {
                return Err(Error::new(
                    format!("{path}.url"),
                    "local Python artifact paths cannot be exported as URLs",
                ));
            };
            let given = data.location.given().unwrap_or_else(|| url.as_str());
            let parsed = super::artifact_url(given, &format!("{path}.url"))?;
            if &parsed != url {
                return Err(Error::new(
                    format!("{path}.url"),
                    "verbatim URL disagrees with the installation artifact URL",
                ));
            }
            // `requires_python` describes wheel compatibility, not the resolved
            // installation, so dropping it changes no installed artifact.
            package.manager = Manager::Pip;
            package.name = data.name.to_string();
            package.version = data.version.to_string();
            package.url = given.to_owned();
            if let Some(hash) = &data.hash {
                package.hash.md5 = hash.md5().map(hex::encode);
                package.hash.sha256 = hash.sha256().map(hex::encode);
            }
            for (index, requirement) in data.requires_dist.iter().enumerate() {
                let dependency_path = format!("{path}.dependencies[{index}]");
                let (name, value) =
                    super::dependencies::python_spec(requirement, &dependency_path)?;
                super::dependencies::insert(
                    &mut package.dependencies,
                    &mut original_dependencies,
                    name,
                    value,
                    dependency_path,
                )?;
            }
        }
    }
    Ok(package)
}
