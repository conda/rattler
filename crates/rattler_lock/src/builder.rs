//! Builder for the creation of lock files. Currently,
//!
use crate::conda::ConversionError;
use crate::{
    content_hash, content_hash::CalculateContentHashError, Channel, CondaLock,
    CondaLockedDependency, GitMeta, LockMeta, LockedDependency, MatchSpec, NoArchType,
    PackageHashes, PackageName, Platform, PypiLockedDependency, RepoDataRecord, TimeMeta,
};
use fxhash::{FxHashMap, FxHashSet};
use rattler_conda_types::{NamelessMatchSpec, PackageUrl};
use std::collections::HashSet;
use url::Url;

/// Struct used to build a conda-lock file
#[derive(Default)]
pub struct LockFileBuilder {
    /// Channels used to resolve dependencies
    pub channels: Vec<Channel>,
    /// The platforms this lock file supports
    pub platforms: FxHashSet<Platform>,
    /// Paths to source files, relative to the parent directory of the lockfile
    pub sources: Option<Vec<String>>,
    /// Metadata dealing with the time lockfile was created
    pub time_metadata: Option<TimeMeta>,
    /// Metadata dealing with the git repo the lockfile was created in and the user that created it
    pub git_metadata: Option<GitMeta>,

    /// Keep track of locked packages per platform
    pub locked_packages: FxHashMap<Platform, LockedPackagesBuilder>,

    /// MatchSpecs input
    /// This is only used to calculate the content_hash
    /// for the lock file
    pub input_specs: Vec<MatchSpec>,
}

impl LockFileBuilder {
    /// Generate a new lock file using the builder pattern
    /// channels, platforms and input_specs need to be provided
    pub fn new(
        channels: impl IntoIterator<Item = impl Into<Channel>>,
        platforms: impl IntoIterator<Item = Platform>,
        input_spec: impl IntoIterator<Item = MatchSpec>,
    ) -> Self {
        Self {
            channels: channels
                .into_iter()
                .map(|into_channel| into_channel.into())
                .collect(),
            platforms: platforms.into_iter().collect(),
            input_specs: input_spec.into_iter().collect(),
            ..Default::default()
        }
    }

    /// Add locked packages per platform
    pub fn add_locked_packages(mut self, locked_packages: LockedPackagesBuilder) -> Self {
        let platform = &locked_packages.platform;
        if self.locked_packages.contains_key(platform) {
            panic!("Tried to insert packages for {platform} twice")
        }

        self.locked_packages
            .insert(locked_packages.platform, locked_packages);
        self
    }

    /// Build a conda_lock file
    pub fn build(self) -> Result<CondaLock, CalculateContentHashError> {
        let content_hash = self
            .platforms
            .iter()
            .map(|plat| {
                Ok((
                    *plat,
                    content_hash::calculate_content_hash(plat, &self.input_specs, &self.channels)?,
                ))
            })
            .collect::<Result<_, CalculateContentHashError>>()?;

        let lock = CondaLock {
            metadata: LockMeta {
                content_hash,
                channels: self.channels,
                platforms: self.platforms.iter().cloned().collect(),
                sources: self.sources.unwrap_or_default(),
                time_metadata: self.time_metadata,
                git_metadata: self.git_metadata,
                inputs_metadata: None,
                custom_metadata: None,
            },
            package: self
                .locked_packages
                .into_values()
                .flat_map(|package| package.build())
                .collect(),
        };
        Ok(lock)
    }
}

/// Shorthand for creating packages per platform
pub struct LockedPackagesBuilder {
    /// The number of locked packages
    pub locked_packages: Vec<LockedDependencyBuilder>,
    /// The to lock the packages to
    pub platform: Platform,
}

pub enum LockedDependencyBuilder {
    Conda(CondaLockedDependencyBuilder),
    Pypi(PypiLockedDependencyBuilder),
}

impl From<CondaLockedDependencyBuilder> for LockedDependencyBuilder {
    fn from(value: CondaLockedDependencyBuilder) -> Self {
        LockedDependencyBuilder::Conda(value)
    }
}

impl From<PypiLockedDependencyBuilder> for LockedDependencyBuilder {
    fn from(value: PypiLockedDependencyBuilder) -> Self {
        LockedDependencyBuilder::Pypi(value)
    }
}

impl LockedPackagesBuilder {
    /// Create a list of locked packages per platform
    pub fn new(platform: Platform) -> Self {
        Self {
            locked_packages: Vec::new(),
            platform,
        }
    }

    /// Add a locked package
    pub fn add_locked_package(&mut self, locked_package: impl Into<LockedDependencyBuilder>) {
        self.locked_packages.push(locked_package.into());
    }

    /// Adds a package and returns self
    pub fn with_locked_package(
        mut self,
        locked_package: impl Into<LockedDependencyBuilder>,
    ) -> Self {
        self.add_locked_package(locked_package);
        self
    }

    /// Transform into list of [`LockedDependency`] objects
    pub fn build(self) -> Vec<LockedDependency> {
        self.locked_packages
            .into_iter()
            .map(|locked_package| match locked_package {
                LockedDependencyBuilder::Conda(locked_package) => LockedDependency {
                    platform: self.platform,
                    version: locked_package.version,
                    name: locked_package.name.as_normalized().to_string(),
                    category: super::default_category(),
                    kind: CondaLockedDependency {
                        dependencies: locked_package.dependency_list,
                        url: locked_package.url,
                        hash: locked_package.package_hashes,
                        source: None,
                        build: Some(locked_package.build),
                        arch: self.platform.arch().map(|arch| arch.to_string()),
                        subdir: Some(self.platform.to_string()),
                        build_number: Some(locked_package.build_number),
                        constrains: locked_package.constrains,
                        features: locked_package.features,
                        track_features: locked_package.track_features,
                        license: locked_package.license,
                        license_family: locked_package.license_family,
                        noarch: locked_package.noarch,
                        size: locked_package.size,
                        timestamp: locked_package.timestamp,
                        purls: locked_package.purls,
                    }
                    .into(),
                },
                LockedDependencyBuilder::Pypi(locked_package) => LockedDependency {
                    platform: self.platform,
                    version: locked_package.version,
                    name: locked_package.name.to_string(),
                    category: super::default_category(),
                    kind: PypiLockedDependency {
                        requires_dist: locked_package.requires_dist,
                        requires_python: locked_package.requires_python,
                        extras: locked_package.extras,
                        url: locked_package.url,
                        hash: locked_package.hash,
                        source: locked_package.source,
                        build: locked_package.build,
                    }
                    .into(),
                },
            })
            .collect()
    }
}

/// Short-hand for creating a LockedPackage that transforms into a [`LockedDependency`]
pub struct CondaLockedDependencyBuilder {
    /// Name of the locked package
    pub name: PackageName,
    /// Package version
    pub version: String,
    /// Package build string
    pub build: String,
    /// Url where the package is hosted
    pub url: Url,
    /// Collection of package hash fields
    pub package_hashes: PackageHashes,
    /// List of dependencies for this package
    pub dependency_list: Vec<String>,
    /// Check if package is optional
    pub optional: Option<bool>,

    /// Experimental: architecture field
    pub arch: Option<String>,

    /// Experimental: the subdir where the package can be found
    pub subdir: Option<String>,

    /// Experimental: conda build number of the package
    pub build_number: u64,

    /// Experimental: see: [Constrains](rattler_conda_types::PackageRecord::constrains)
    pub constrains: Vec<String>,

    /// Experimental: see: [Features](rattler_conda_types::PackageRecord::features)
    pub features: Option<String>,

    /// Experimental: see: [Track features](rattler_conda_types::PackageRecord::track_features)
    pub track_features: Vec<String>,

    /// Experimental: the specific license of the package
    pub license: Option<String>,

    /// Experimental: the license family of the package
    pub license_family: Option<String>,

    /// Experimental: If this package is independent of architecture this field specifies in what way. See
    /// [`NoArchType`] for more information.
    pub noarch: NoArchType,

    /// Experimental: The size of the package archive in bytes
    pub size: Option<u64>,

    /// Experimental: The date this entry was created.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Experimental: Defines that the package is an alias for a package from another ecosystem.
    pub purls: Vec<PackageUrl>,
}

impl TryFrom<&RepoDataRecord> for CondaLockedDependencyBuilder {
    type Error = ConversionError;

    fn try_from(value: &RepoDataRecord) -> Result<Self, Self::Error> {
        Self::try_from(value.clone())
    }
}

impl TryFrom<RepoDataRecord> for CondaLockedDependencyBuilder {
    type Error = ConversionError;

    fn try_from(record: RepoDataRecord) -> Result<Self, Self::Error> {
        // Generate hashes
        let hashes =
            PackageHashes::from_hashes(record.package_record.md5, record.package_record.sha256);
        let hashes = hashes.ok_or_else(|| ConversionError::Missing("md5 or sha265".to_string()))?;

        Ok(Self {
            name: record.package_record.name,
            version: record.package_record.version.to_string(),
            build: record.package_record.build,
            url: record.url,
            package_hashes: hashes,
            dependency_list: record.package_record.depends,
            optional: None,
            arch: record.package_record.arch,
            subdir: Some(record.package_record.subdir),
            build_number: record.package_record.build_number,
            constrains: record.package_record.constrains,
            features: record.package_record.features,
            track_features: record.package_record.track_features,
            license: record.package_record.license,
            license_family: record.package_record.license_family,
            noarch: record.package_record.noarch,
            size: record.package_record.size,
            timestamp: record.package_record.timestamp,
            purls: record.package_record.purls,
        })
    }
}

impl CondaLockedDependencyBuilder {
    /// Set if the package should be optional
    pub fn set_optional(mut self, optional: bool) -> Self {
        self.optional = Some(optional);
        self
    }

    /// Add a single dependency
    pub fn add_dependency(
        mut self,
        key: PackageName,
        version_constraint: NamelessMatchSpec,
    ) -> Self {
        self.dependency_list
            .push(MatchSpec::from_nameless(version_constraint, Some(key)).to_string());
        self
    }

    /// Add multiple dependencies
    pub fn add_dependencies(
        mut self,
        value: impl IntoIterator<Item = (PackageName, NamelessMatchSpec)>,
    ) -> Self {
        self.dependency_list.extend(
            value
                .into_iter()
                .map(|(n, spec)| MatchSpec::from_nameless(spec, Some(n)).to_string()),
        );
        self
    }

    /// Set the subdir for for the package
    pub fn set_arch<S: AsRef<str>>(mut self, arch: String) -> Self {
        self.subdir = Some(arch);
        self
    }

    /// Set the subdir for for the package
    pub fn set_subdir<S: AsRef<str>>(mut self, subdir: String) -> Self {
        self.subdir = Some(subdir);
        self
    }

    /// Set the subdir for for the package
    pub fn set_build_number<S: AsRef<str>>(mut self, build_number: u64) -> Self {
        self.build_number = build_number;
        self
    }

    /// Add the constrains for this package
    pub fn add_constrain<S: AsRef<str>>(mut self, constrain: S) -> Self {
        self.constrains.push(constrain.as_ref().to_string());
        self
    }

    /// Add the constrains for this package
    pub fn add_constrains<S: AsRef<str>>(
        mut self,
        constrain: impl IntoIterator<Item = String>,
    ) -> Self {
        self.constrains.extend(constrain);
        self
    }

    /// Set the features for for the package
    pub fn set_features<S: AsRef<str>>(mut self, features: S) -> Self {
        self.features = Some(features.as_ref().to_string());
        self
    }

    /// Add a track feature for the package
    pub fn add_track_feature<S: AsRef<str>>(mut self, track_feature: S) -> Self {
        self.track_features.push(track_feature.as_ref().to_string());
        self
    }

    /// Add multiple track features for for the package
    pub fn add_track_features(mut self, value: impl IntoIterator<Item = String>) -> Self {
        self.track_features.extend(value);
        self
    }

    /// Set the licence for for the package
    pub fn add_license<S: AsRef<str>>(mut self, license: S) -> Self {
        self.license = Some(license.as_ref().to_string());
        self
    }

    /// Set the license family for for the package
    pub fn add_license_family<S: AsRef<str>>(mut self, license_family: S) -> Self {
        self.license_family = Some(license_family.as_ref().to_string());
        self
    }

    /// Set the noarch type for for the package
    pub fn add_noarch(mut self, noarch_type: NoArchType) -> Self {
        self.noarch = noarch_type;
        self
    }

    /// Set the size of the package
    pub fn set_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Set the timestamp of the package
    pub fn set_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Adds a PackageUrl to the package
    pub fn add_purl(mut self, purl: PackageUrl) -> Self {
        self.purls.push(purl);
        self
    }
}

pub struct PypiLockedDependencyBuilder {
    /// Name of the locked package
    pub name: String,
    /// Package version
    pub version: String,

    /// A list of dependencies on other packages that the wheel listed.
    pub requires_dist: Vec<String>,

    /// The python version that this package requires.
    pub requires_python: Option<String>,

    /// A list of extras that are selected
    pub extras: HashSet<String>,

    /// The URL that points to where the artifact can be downloaded from.
    pub url: Url,

    /// Hashes of the file pointed to by `url`.
    pub hash: Option<PackageHashes>,

    /// ???
    pub source: Option<Url>,

    /// Build string
    pub build: Option<String>,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use std::str::FromStr;

    use crate::builder::{CondaLockedDependencyBuilder, LockFileBuilder, LockedPackagesBuilder};
    use crate::PackageHashes;
    use rattler_conda_types::{
        ChannelConfig, MatchSpec, NoArchType, PackageName, Platform, RepoDataRecord,
    };
    use rattler_digest::parse_digest_from_hex;

    #[test]
    fn conda_lock_builder_and_conversions() {
        let _channel_config = ChannelConfig::default();
        let lock = LockFileBuilder::new(
            ["conda_forge"],
            [Platform::Osx64],
            [MatchSpec::from_str("python =3.11.0").unwrap()]
        )
            .add_locked_packages(LockedPackagesBuilder::new(Platform::Osx64)
                .with_locked_package(CondaLockedDependencyBuilder {
                    name: PackageName::new_unchecked("python"),
                    version: "3.11.0".to_string(),
                    build: "h4150a38_1_cpython".to_string(),
                    url: "https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda".parse().unwrap(),
                    package_hashes:  PackageHashes::Md5Sha256(parse_digest_from_hex::<rattler_digest::Md5>("c6f4b87020c72e2700e3e94c1fc93b70").unwrap(),
                                                               parse_digest_from_hex::<rattler_digest::Sha256>("7c58de8c7d98b341bd9be117feec64782e704fec5c30f6e14713ebccaab9b5d8").unwrap()),
                    dependency_list: vec![String::from("python 3.11.0.*")],
                    optional: None,
                    arch: Some("x86_64".to_string()),
                    subdir: Some("noarch".to_string()),
                    build_number: 12,
                    constrains: vec!["bla".to_string()],
                    features: Some("foobar".to_string()),
                    track_features: vec!["dont-track".to_string()],
                    license: Some("BSD-3-Clause".to_string()),
                    license_family: Some("BSD".to_string()),
                    noarch: NoArchType::python(),
                    size: Some(12000),
                    timestamp: Some(Utc::now()),
                    purls: vec![
                        "pkg:deb/debian/python@3.11.0?arch=x86_64".parse().unwrap(),
                    ]
                }))
            .build().unwrap();

        // Convert to RepoDataRecord
        let locked_dep = lock.package.first().unwrap();
        let record = RepoDataRecord::try_from(locked_dep).unwrap();

        assert_eq!(
            record.package_record.name.as_source(),
            locked_dep.name.as_str()
        );
        assert_eq!(
            record.channel,
            "https://conda.anaconda.org/conda-forge".to_string()
        );
        assert_eq!(
            record.file_name,
            "python-3.11.0-h4150a38_1_cpython.conda".to_string()
        );
        assert_eq!(
            record.package_record.version.to_string(),
            locked_dep.version
        );
        assert_eq!(
            Some(&record.package_record.build),
            locked_dep.as_conda().unwrap().build.as_ref()
        );
        assert_eq!(
            record.package_record.platform.clone().unwrap(),
            locked_dep.platform.only_platform().unwrap()
        );
        assert_eq!(
            record.package_record.arch,
            locked_dep.as_conda().unwrap().arch
        );
        assert_eq!(
            Some(&record.package_record.subdir),
            locked_dep.as_conda().unwrap().subdir.as_ref()
        );
        assert_eq!(
            Some(record.package_record.build_number),
            locked_dep.as_conda().unwrap().build_number
        );
        assert_eq!(
            record.package_record.constrains,
            locked_dep.as_conda().unwrap().constrains.clone()
        );
        assert_eq!(
            record.package_record.features,
            locked_dep.as_conda().unwrap().features
        );
        assert_eq!(
            record.package_record.track_features,
            locked_dep.as_conda().unwrap().track_features
        );
        assert_eq!(
            record.package_record.license_family,
            locked_dep.as_conda().unwrap().license_family
        );
        assert_eq!(
            record.package_record.noarch,
            locked_dep.as_conda().unwrap().noarch
        );
        assert_eq!(
            record.package_record.size,
            locked_dep.as_conda().unwrap().size
        );
        assert_eq!(
            record.package_record.timestamp,
            locked_dep.as_conda().unwrap().timestamp
        );

        // Convert to LockedDependency
        let locked_package = CondaLockedDependencyBuilder::try_from(record.clone()).unwrap();
        assert_eq!(record.package_record.name, locked_package.name);
        assert_eq!(
            record.package_record.version.to_string(),
            locked_package.version
        );
        assert_eq!(&record.package_record.build, &locked_package.build);
        assert_eq!(record.package_record.arch, locked_package.arch);
        assert_eq!(
            record.package_record.subdir,
            locked_package.subdir.clone().unwrap_or_default()
        );
        assert_eq!(
            record.package_record.build_number,
            locked_package.build_number
        );
        assert_eq!(record.package_record.constrains, locked_package.constrains);
        assert_eq!(record.package_record.features, locked_package.features);
        assert_eq!(
            record.package_record.license_family,
            locked_package.license_family
        );
        assert_eq!(record.package_record.noarch, locked_package.noarch);
        assert_eq!(record.package_record.size, locked_package.size);
        assert_eq!(record.package_record.timestamp, locked_package.timestamp);
    }
}
