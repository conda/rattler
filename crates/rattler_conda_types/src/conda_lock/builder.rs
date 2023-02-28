use crate::conda_lock::{
    content_hash, Channel, CondaLock, GitMeta, LockMeta, LockedDependency, Manager, PackageHashes,
    TimeMeta, VersionConstraint,
};
use crate::{MatchSpec, Platform};
use std::collections::{HashMap, HashSet};
use url::Url;

/// Struct used to build a conda-lock file
#[derive(Default)]
struct LockFileBuilder {
    /// Channels used to resolve dependencies
    pub channels: Vec<Channel>,
    /// The platforms this lock file supports
    pub platforms: HashSet<Platform>,
    /// Paths to source files, relative to the parent directory of the lockfile
    pub sources: Option<Vec<String>>,
    /// Metadata dealing with the time lockfile was created
    pub time_metadata: Option<TimeMeta>,
    /// Metadata dealing with the git repo the lockfile was created in and the user that created it
    pub git_metadata: Option<GitMeta>,

    /// Keep track of locked packages per platform
    pub locked_packages: HashMap<Platform, LockedPackages>,

    /// MatchSpecs input
    /// This is only used to calculate the content_hash
    /// for the lock file
    pub input_specs: Vec<MatchSpec>,
}

impl LockFileBuilder {
    pub fn new(
        channels: impl IntoIterator<Item = Channel>,
        platforms: impl IntoIterator<Item = Platform>,
        input_spec: impl IntoIterator<Item = MatchSpec>,
    ) -> Self {
        Self {
            channels: channels
                .into_iter()
                .map(|into_channel| into_channel.into())
                .collect(),
            platforms: platforms
                .into_iter()
                .collect(),
            input_specs: input_spec.into_iter().collect(),
            ..Default::default()
        }
    }

    /// Add locked packages per platform
    pub fn add_locked_packages(mut self, locked_packages: LockedPackages) -> Self {
        self.locked_packages
            .insert(locked_packages.platform, locked_packages);
        self
    }

    /// Build a conda_lock file
    pub fn build(self) -> CondaLock {
        CondaLock {
            metadata: LockMeta {
                content_hash: self
                    .platforms
                    .iter()
                    .map(|plat| {
                        (
                            *plat,
                            content_hash::calculate_content_hash(
                                plat,
                                &self.input_specs,
                                &self.channels,
                            ),
                        )
                    })
                    .collect(),
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
            version: super::default_version(),
        }
    }
}

/// Shorthand for creating packages per platform
struct LockedPackages {
    pub locked_packages: Vec<LockedPackage>,
    pub platform: Platform,
}

impl LockedPackages {
    pub fn new(platform: Platform) -> Self {
        Self {
            locked_packages: Vec::new(),
            platform,
        }
    }

    /// Add a locked package
    pub fn add_locked_package(mut self, locked_package: LockedPackage) -> Self {
        self.locked_packages.push(locked_package);
        self
    }

    /// Transform into list of [`LockedDependency`] objects
    pub fn build(self) -> Vec<LockedDependency> {
        self.locked_packages
            .into_iter()
            .map(|locked_package| {
                LockedDependency {
                    name: locked_package.name,
                    version: locked_package.version,
                    /// Use conda as default manager for now
                    manager: Manager::Conda,
                    platform: self.platform,
                    dependencies: locked_package.dependency_list,
                    url: locked_package.url,
                    hash: locked_package.package_hashes,
                    optional: locked_package.optional.unwrap_or(false),
                    category: super::default_category(),
                    source: None,
                    build: Some(locked_package.build_string),
                }
            })
            .collect()
    }
}

/// Short-hand for creating a LockedPackage that transforms into a [`LockedDependency`]
struct LockedPackage {
    pub name: String,
    pub version: String,
    pub build_string: String,
    pub url: Url,
    pub package_hashes: PackageHashes,
    pub dependency_list: HashMap<String, VersionConstraint>,
    pub optional: Option<bool>,
}

impl LockedPackage {
    /// Set if the package should be optional
    pub fn set_optional(mut self, optional: bool) -> Self {
        self.optional = Some(optional);
        self
    }

    /// Add a single dependency
    pub fn add_dependency<S: AsRef<str>>(
        mut self,
        key: S,
        version_constraint: VersionConstraint,
    ) -> Self {
        self.dependency_list
            .insert(key.as_ref().to_string(), version_constraint);
        self
    }

    /// Add multiple dependencies
    pub fn add_dependencies(
        mut self,
        value: impl IntoIterator<Item = (String, VersionConstraint)>,
    ) -> Self {
        self.dependency_list.extend(value);
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::conda_lock::builder::{LockFileBuilder, LockedPackage, LockedPackages};
    use crate::conda_lock::{Channel, PackageHashes};
    use crate::{ChannelConfig, MatchSpec, Platform};

    #[test]
    fn create_lock_file() {
        let channel_config = ChannelConfig::default();
        let lock = LockFileBuilder::new(
            ["conda_forge".into()],
            [Platform::Osx64],
            [MatchSpec::from_str("python =3.11.0", &channel_config).unwrap()]
        )
            .add_locked_packages(LockedPackages::new(Platform::Osx64)
                .add_locked_package(LockedPackage {
                    name: "python".to_string(),
                    version: "3.11.0".to_string(),
                    build_string: "h4150a38_1_cpython".to_string(),
                    url: "https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda".parse().unwrap(),
                    package_hashes:  PackageHashes::Md5Sha256(rattler_digest::parse_digest_from_hex::<md5::Md5>("c6f4b87020c72e2700e3e94c1fc93b70").unwrap(),
                                                               rattler_digest::parse_digest_from_hex::<sha2::Sha256>("7c58de8c7d98b341bd9be117feec64782e704fec5c30f6e14713ebccaab9b5d8").unwrap()),
                    dependency_list: Default::default(),
                    optional: None,
                }))
            .build();
    }

    //
    // md5: c6f4b87020c72e2700e3e94c1fc93b70
    // sha256: 7c58de8c7d98b341bd9be117feec64782e704fec5c30f6e14713ebccaab9b5d8
}
