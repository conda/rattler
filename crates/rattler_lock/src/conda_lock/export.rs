use std::collections::BTreeMap;

use rattler_conda_lock::{
    Channel, Hashes, LockFile as CepLockFile, Manager, Metadata, NodePath, Package,
};

use super::dependencies::{Dependencies, conda_spec, python_spec};
use super::error::{CondaLockError, CondaLockErrorKind};
use crate::utils::derived_fields::{LocationDerivedFields, derive_noarch_type};
use crate::{CondaPackageData, Environment, LockedPackage, PypiPackageData, UrlOrPath};

/// Metadata supplied by the caller when exporting a single environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Original specification filenames. Defaults to an empty required CEP-37 list.
    pub sources: Vec<String>,
    /// Exact per-platform content hashes. When absent, canonical package hashes
    /// are computed; these are not conda-lock's original input-specification hashes.
    pub content_hash: Option<BTreeMap<String, String>>,
}

impl Environment<'_> {
    /// Exports this environment as nonoptional `main` CEP-37 packages, entirely
    /// offline.
    ///
    /// Solver-only repodata and provenance may be omitted. Source builds and
    /// install semantics that CEP-37 cannot represent are errors. Channel order
    /// is preserved.
    ///
    /// ```
    /// # fn example(environment: rattler_lock::Environment<'_>) -> Result<(), rattler_lock::conda_lock::CondaLockError> {
    /// use rattler_lock::conda_lock::ExportOptions;
    ///
    /// let options = ExportOptions {
    ///     sources: vec!["environment.yml".into()],
    ///     ..Default::default()
    /// };
    /// let cep = environment.to_conda_lock(&options)?;
    /// let yaml = cep.to_yaml()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn to_conda_lock(&self, options: &ExportOptions) -> Result<CepLockFile, CondaLockError> {
        let mut result = CepLockFile {
            metadata: Metadata {
                channels: self
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
        let platforms_path = NodePath::root().field("metadata").field("platforms");
        let packages_path = NodePath::root().field("package");
        let mut platforms: Vec<_> = self.platforms().collect();
        platforms.sort_by(|left, right| left.name().as_str().cmp(right.name().as_str()));
        let mut subdirs = BTreeMap::new();
        for (platform_index, platform) in platforms.into_iter().enumerate() {
            let platform_path = platforms_path.index(platform_index);
            let subdir = platform.subdir().to_string();
            if let Some(original) = subdirs.insert(subdir.clone(), platform_path.clone()) {
                return Err(CondaLockError::new(
                    platform_path,
                    CondaLockErrorKind::CollapsingPlatforms,
                )
                .with_related_path(original, "first platform with this subdir"));
            }
            if platform.name().as_str() != subdir {
                return Err(CondaLockError::new(
                    platform_path,
                    CondaLockErrorKind::CustomPlatformName,
                ));
            }
            if !platform.virtual_packages().is_empty() {
                return Err(CondaLockError::new(
                    platform_path,
                    CondaLockErrorKind::VirtualPackages,
                ));
            }
            result.metadata.platforms.push(subdir.clone());
            let mut names = BTreeMap::new();
            for locked in self.packages(platform).into_iter().flatten() {
                let path = packages_path.index(result.package.len());
                let package = convert(locked, &subdir, &path)?;
                let normalized = match locked {
                    LockedPackage::Conda(data) => data.name().as_normalized().to_owned(),
                    LockedPackage::Pypi(_) => package.name.clone(),
                };
                if let Some(original) = names.insert((package.manager, normalized), path.clone()) {
                    return Err(CondaLockError::new(
                        path,
                        CondaLockErrorKind::DuplicateExportedName,
                    )
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
}

fn convert(
    locked: &LockedPackage,
    platform: &str,
    path: &NodePath,
) -> Result<Package, CondaLockError> {
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
    let url_path = path.field("url");
    let mut dependencies = Dependencies::default();
    match locked {
        LockedPackage::Conda(CondaPackageData::Source(_)) => {
            return Err(CondaLockError::new(
                path.clone(),
                CondaLockErrorKind::CondaSourceBuild,
            ));
        }
        LockedPackage::Conda(CondaPackageData::Binary(data)) => {
            let record = &data.package_record;
            let UrlOrPath::Url(url) = &data.location else {
                return Err(CondaLockError::new(
                    url_path,
                    CondaLockErrorKind::LocalArtifactPath,
                ));
            };
            super::artifact_url(url.as_str(), &url_path)?;
            let derived = LocationDerivedFields::new(&data.location);
            if derived.identifier.as_ref() != Some(&data.file_name) {
                return Err(CondaLockError::new(
                    url_path,
                    CondaLockErrorKind::UnreconstructableFileName,
                ));
            }
            // CEP-37 stores no subdir, so a reader derives it from the URL. Only
            // a record naming a *different* target would be silently relabelled;
            // the target platform itself is an accepted spelling for noarch.
            let reconstructed_subdir = derived.subdir.as_deref().unwrap_or(platform);
            if reconstructed_subdir != record.subdir && record.subdir != platform {
                return Err(CondaLockError::new(
                    path.field("platform"),
                    CondaLockErrorKind::SubdirDisagreement,
                ));
            }
            if record.noarch != derive_noarch_type(reconstructed_subdir, &record.build) {
                return Err(CondaLockError::new(
                    url_path,
                    CondaLockErrorKind::NoarchMismatch,
                ));
            }
            if record.python_site_packages_path.is_some() {
                return Err(CondaLockError::new(
                    path.clone(),
                    CondaLockErrorKind::CustomSitePackagesPath,
                ));
            }
            if !record.extra_depends.is_empty() {
                return Err(CondaLockError::new(
                    path.field("dependencies"),
                    CondaLockErrorKind::ExtraDepends,
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
            let dependencies_path = path.field("dependencies");
            for (index, dependency) in record.depends.iter().enumerate() {
                let dependency_path = dependencies_path.index(index);
                let converted = conda_spec(dependency, &dependency_path)?;
                dependencies.insert(converted, dependency_path)?;
            }
        }
        LockedPackage::Pypi(PypiPackageData::Source(_)) => {
            return Err(CondaLockError::new(
                path.clone(),
                CondaLockErrorKind::PythonSourceTree,
            ));
        }
        LockedPackage::Pypi(PypiPackageData::Distribution(data)) => {
            let UrlOrPath::Url(url) = data.location.inner() else {
                return Err(CondaLockError::new(
                    url_path,
                    CondaLockErrorKind::LocalArtifactPath,
                ));
            };
            let given = data.location.given().unwrap_or_else(|| url.as_str());
            let parsed = super::artifact_url(given, &url_path)?;
            if &parsed != url {
                return Err(CondaLockError::new(
                    url_path,
                    CondaLockErrorKind::VerbatimUrlMismatch,
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
            let dependencies_path = path.field("dependencies");
            for (index, requirement) in data.requires_dist.iter().enumerate() {
                let dependency_path = dependencies_path.index(index);
                let converted = python_spec(requirement, &dependency_path)?;
                dependencies.insert(converted, dependency_path)?;
            }
        }
    }
    package.dependencies = dependencies.into_values();
    Ok(package)
}
