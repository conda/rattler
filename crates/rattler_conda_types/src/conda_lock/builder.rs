//! Builder for the creation of lock files. Currently,
//!
use crate::conda_lock::content_hash::CalculateContentHashError;
use crate::conda_lock::{
    content_hash, Channel, CondaLock, GitMeta, LockMeta, LockedDependency, Manager, PackageHashes,
    TimeMeta, VersionConstraint,
};
use crate::{MatchSpec, Platform};
use std::collections::{HashMap, HashSet};
use url::Url;

/// Struct used to build a conda-lock file
#[derive(Default)]
pub struct LockFileBuilder {
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
    pub fn add_locked_packages(mut self, locked_packages: LockedPackages) -> Self {
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
            .collect::<Result<HashMap<_, _>, CalculateContentHashError>>()?;

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
            version: super::default_version(),
        };
        Ok(lock)
    }
}

/// Shorthand for creating packages per platform
pub struct LockedPackages {
    /// The number of locked packages
    pub locked_packages: Vec<LockedPackage>,
    /// The to lock the packages to
    pub platform: Platform,
}

impl LockedPackages {
    /// Create a list of locked packages per platform
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
pub struct LockedPackage {
    /// Name of the locked package
    pub name: String,
    /// Package version
    pub version: String,
    /// Package build string
    pub build_string: String,
    /// Url where the package is hosted
    pub url: Url,
    /// Collection of package hash fields
    pub package_hashes: PackageHashes,
    /// List of dependencies for this package
    pub dependency_list: HashMap<String, VersionConstraint>,
    /// Check if package is optional
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
    use std::str::FromStr;

    use crate::conda_lock::builder::{LockFileBuilder, LockedPackage, LockedPackages};
    use crate::conda_lock::{CondaLock, PackageHashes};
    use crate::{ChannelConfig, MatchSpec, Platform};
    use rattler_digest::parse_digest_from_hex;

    #[test]
    fn create_lock_file() {
        let _channel_config = ChannelConfig::default();
        let lock = LockFileBuilder::new(
            ["conda_forge"],
            [Platform::Osx64],
            [MatchSpec::from_str("python =3.11.0").unwrap()]
        )
            .add_locked_packages(LockedPackages::new(Platform::Osx64)
                .add_locked_package(LockedPackage {
                    name: "python".to_string(),
                    version: "3.11.0".to_string(),
                    build_string: "h4150a38_1_cpython".to_string(),
                    url: "https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda".parse().unwrap(),
                    package_hashes:  PackageHashes::Md5Sha256(parse_digest_from_hex::<rattler_digest::Md5>("c6f4b87020c72e2700e3e94c1fc93b70").unwrap(),
                                                               parse_digest_from_hex::<rattler_digest::Sha256>("7c58de8c7d98b341bd9be117feec64782e704fec5c30f6e14713ebccaab9b5d8").unwrap()),
                    dependency_list: Default::default(),
                    optional: None,
                }))
            .build().unwrap();

        // See if we can serialize/deserialize it
        let s = serde_yaml::to_string(&lock).unwrap();
        serde_yaml::from_str::<CondaLock>(&s).unwrap();
    }
}
