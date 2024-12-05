//! Builder for the creation of lock files.

use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use fxhash::FxHashMap;
use indexmap::{IndexMap, IndexSet};
use pep508_rs::ExtraName;
use rattler_conda_types::{Platform, Version};

use crate::{
    file_format_version::FileFormatVersion, Channel, CondaBinaryData, CondaPackageData,
    CondaSourceData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    LockedPackageRef, PypiIndexes, PypiPackageData, PypiPackageEnvironmentData, UrlOrPath,
};

/// Information about a single locked package in an environment.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum LockedPackage {
    /// A conda package
    Conda(CondaPackageData),

    /// A pypi package in an environment
    Pypi(PypiPackageData, PypiPackageEnvironmentData),
}

impl From<LockedPackageRef<'_>> for LockedPackage {
    fn from(value: LockedPackageRef<'_>) -> Self {
        match value {
            LockedPackageRef::Conda(data) => LockedPackage::Conda(data.clone()),
            LockedPackageRef::Pypi(data, env) => LockedPackage::Pypi(data.clone(), env.clone()),
        }
    }
}

impl From<CondaPackageData> for LockedPackage {
    fn from(value: CondaPackageData) -> Self {
        LockedPackage::Conda(value)
    }
}

impl From<(PypiPackageData, PypiPackageEnvironmentData)> for LockedPackage {
    fn from((data, env): (PypiPackageData, PypiPackageEnvironmentData)) -> Self {
        LockedPackage::Pypi(data, env)
    }
}

impl LockedPackage {
    /// Returns the name of the package as it occurs in the lock file. This
    /// might not be the normalized name.
    pub fn name(&self) -> &str {
        match self {
            LockedPackage::Conda(data) => data.record().name.as_source(),
            LockedPackage::Pypi(data, _) => data.name.as_ref(),
        }
    }

    /// Returns the location of the package.
    pub fn location(&self) -> &UrlOrPath {
        match self {
            LockedPackage::Conda(data) => data.location(),
            LockedPackage::Pypi(data, _) => &data.location,
        }
    }

    /// Returns the conda package data if this is a conda package.
    pub fn as_conda(&self) -> Option<&CondaPackageData> {
        match self {
            LockedPackage::Conda(data) => Some(data),
            LockedPackage::Pypi(..) => None,
        }
    }

    /// Returns the pypi package data if this is a pypi package.
    pub fn as_pypi(&self) -> Option<(&PypiPackageData, &PypiPackageEnvironmentData)> {
        match self {
            LockedPackage::Conda(..) => None,
            LockedPackage::Pypi(data, env) => Some((data, env)),
        }
    }

    /// Returns the package as a binary conda package if this is a binary conda
    /// package.
    pub fn as_binary_conda(&self) -> Option<&CondaBinaryData> {
        self.as_conda().and_then(CondaPackageData::as_binary)
    }

    /// Returns the package as a source conda package if this is a source conda
    /// package.
    pub fn as_source_conda(&self) -> Option<&CondaSourceData> {
        self.as_conda().and_then(CondaPackageData::as_source)
    }

    /// Returns the conda package data if this is a conda package.
    pub fn into_conda(self) -> Option<CondaPackageData> {
        match self {
            LockedPackage::Conda(data) => Some(data),
            LockedPackage::Pypi(..) => None,
        }
    }

    /// Returns the pypi package data if this is a pypi package.
    pub fn into_pypi(self) -> Option<(PypiPackageData, PypiPackageEnvironmentData)> {
        match self {
            LockedPackage::Conda(..) => None,
            LockedPackage::Pypi(data, env) => Some((data, env)),
        }
    }
}

/// A struct to incrementally build a lock-file.
#[derive(Default)]
pub struct LockFileBuilder {
    /// Metadata about the different environments stored in the lock file.
    environments: IndexMap<String, EnvironmentData>,

    /// A list of all package metadata stored in the lock file.
    conda_packages: IndexMap<UniqueCondaIdentifier, CondaPackageData>,
    pypi_packages: IndexSet<PypiPackageData>,
    pypi_runtime_configurations: IndexSet<HashablePypiPackageEnvironmentData>,
}

/// A unique identifier for a conda package. This is used to deduplicate
/// packages. This only includes the unique identifying aspects of a package.
#[derive(Debug, Hash, Eq, PartialEq)]
struct UniqueCondaIdentifier {
    location: UrlOrPath,
    normalized_name: String,
    version: Version,
    build: String,
    subdir: String,
}

impl<'a> From<&'a CondaPackageData> for UniqueCondaIdentifier {
    fn from(value: &'a CondaPackageData) -> Self {
        Self {
            location: value.location().clone(),
            normalized_name: value.record().name.as_normalized().to_string(),
            version: value.record().version.version().clone(),
            build: value.record().build.clone(),
            subdir: value.record().subdir.clone(),
        }
    }
}

impl LockFileBuilder {
    /// Generate a new lock file using the builder pattern
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the pypi indexes for an environment.
    pub fn set_pypi_indexes(
        &mut self,
        environment_data: impl Into<String>,
        indexes: PypiIndexes,
    ) -> &mut Self {
        self.environments
            .entry(environment_data.into())
            .or_insert_with(|| EnvironmentData {
                channels: vec![],
                packages: FxHashMap::default(),
                indexes: None,
            })
            .indexes = Some(indexes);
        self
    }

    /// Sets the metadata for an environment.
    pub fn set_channels(
        &mut self,
        environment: impl Into<String>,
        channels: impl IntoIterator<Item = impl Into<Channel>>,
    ) -> &mut Self {
        self.environments
            .entry(environment.into())
            .or_insert_with(|| EnvironmentData {
                channels: vec![],
                packages: FxHashMap::default(),
                indexes: None,
            })
            .channels = channels.into_iter().map(Into::into).collect();
        self
    }

    /// Adds a conda locked package to a specific environment and platform.
    ///
    /// This function is similar to [`Self::with_conda_package`] but differs in
    /// that it takes a mutable reference to self instead of consuming it.
    /// This allows for a more fluent with chaining calls.
    pub fn add_conda_package(
        &mut self,
        environment: impl Into<String>,
        platform: Platform,
        locked_package: CondaPackageData,
    ) -> &mut Self {
        // Get the environment
        let environment = self
            .environments
            .entry(environment.into())
            .or_insert_with(|| EnvironmentData {
                channels: vec![],
                packages: HashMap::default(),
                indexes: None,
            });

        let unique_identifier = UniqueCondaIdentifier::from(&locked_package);

        // Add the package to the list of packages.
        let entry = self.conda_packages.entry(unique_identifier);
        let package_idx = entry.index();
        entry
            .and_modify(|pkg| {
                if let Cow::Owned(merged_package) = pkg.merge(&locked_package) {
                    *pkg = merged_package;
                }
            })
            .or_insert(locked_package);

        // Add the package to the environment that it is intended for.
        environment
            .packages
            .entry(platform)
            .or_default()
            .insert(EnvironmentPackageData::Conda(package_idx));

        self
    }

    /// Adds a pypi locked package to a specific environment and platform.
    ///
    /// This function is similar to [`Self::with_pypi_package`] but differs in
    /// that it takes a mutable reference to self instead of consuming it.
    /// This allows for a more fluent with chaining calls.
    pub fn add_pypi_package(
        &mut self,
        environment: impl Into<String>,
        platform: Platform,
        locked_package: PypiPackageData,
        environment_data: PypiPackageEnvironmentData,
    ) -> &mut Self {
        // Get the environment
        let environment = self
            .environments
            .entry(environment.into())
            .or_insert_with(|| EnvironmentData {
                channels: vec![],
                packages: HashMap::default(),
                indexes: None,
            });

        // Add the package to the list of packages.
        let package_idx = self.pypi_packages.insert_full(locked_package).0;
        let runtime_idx = self
            .pypi_runtime_configurations
            .insert_full(environment_data.into())
            .0;

        // Add the package to the environment that it is intended for.
        environment
            .packages
            .entry(platform)
            .or_default()
            .insert(EnvironmentPackageData::Pypi(package_idx, runtime_idx));

        self
    }

    /// Adds a conda locked package to a specific environment and platform.
    ///
    /// This function is similar to [`Self::add_conda_package`] but differs in
    /// that it consumes `self` instead of taking a mutable reference. This
    /// allows for a better interface when modifying an existing instance.
    pub fn with_conda_package(
        mut self,
        environment: impl Into<String>,
        platform: Platform,
        locked_package: CondaPackageData,
    ) -> Self {
        self.add_conda_package(environment, platform, locked_package);
        self
    }

    /// Adds a package from another environment to a specific environment and
    /// platform.
    pub fn with_package(
        mut self,
        environment: impl Into<String>,
        platform: Platform,
        locked_package: LockedPackage,
    ) -> Self {
        self.add_package(environment, platform, locked_package);
        self
    }

    /// Adds a package from another environment to a specific environment and
    /// platform.
    pub fn add_package(
        &mut self,
        environment: impl Into<String>,
        platform: Platform,
        locked_package: LockedPackage,
    ) -> &mut Self {
        match locked_package {
            LockedPackage::Conda(p) => self.add_conda_package(environment, platform, p),
            LockedPackage::Pypi(data, env_data) => {
                self.add_pypi_package(environment, platform, data, env_data)
            }
        }
    }

    /// Adds a pypi locked package to a specific environment and platform.
    ///
    /// This function is similar to [`Self::add_pypi_package`] but differs in
    /// that it consumes `self` instead of taking a mutable reference. This
    /// allows for a better interface when modifying an existing instance.
    pub fn with_pypi_package(
        mut self,
        environment: impl Into<String>,
        platform: Platform,
        locked_package: PypiPackageData,
        environment_data: PypiPackageEnvironmentData,
    ) -> Self {
        self.add_pypi_package(environment, platform, locked_package, environment_data);
        self
    }

    /// Sets the channels of an environment.
    pub fn with_channels(
        mut self,
        environment: impl Into<String>,
        channels: impl IntoIterator<Item = impl Into<Channel>>,
    ) -> Self {
        self.set_channels(environment, channels);
        self
    }

    /// Sets the channels of an environment.
    pub fn with_pypi_indexes(
        mut self,
        environment: impl Into<String>,
        indexes: PypiIndexes,
    ) -> Self {
        self.set_pypi_indexes(environment, indexes);
        self
    }

    /// Build a [`LockFile`]
    pub fn finish(self) -> LockFile {
        let (environment_lookup, environments) = self
            .environments
            .into_iter()
            .enumerate()
            .map(|(idx, (name, env))| ((name, idx), env))
            .unzip();

        LockFile {
            inner: Arc::new(LockFileInner {
                version: FileFormatVersion::LATEST,
                conda_packages: self.conda_packages.into_values().collect(),
                pypi_packages: self.pypi_packages.into_iter().collect(),
                pypi_environment_package_data: self
                    .pypi_runtime_configurations
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                environments,
                environment_lookup,
            }),
        }
    }
}

/// Similar to [`PypiPackageEnvironmentData`] but hashable.
#[derive(Hash, PartialEq, Eq)]
struct HashablePypiPackageEnvironmentData {
    extras: BTreeSet<ExtraName>,
}

impl From<HashablePypiPackageEnvironmentData> for PypiPackageEnvironmentData {
    fn from(value: HashablePypiPackageEnvironmentData) -> Self {
        Self {
            extras: value.extras.into_iter().collect(),
        }
    }
}

impl From<PypiPackageEnvironmentData> for HashablePypiPackageEnvironmentData {
    fn from(value: PypiPackageEnvironmentData) -> Self {
        Self {
            extras: value.extras.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use rattler_conda_types::{PackageName, PackageRecord, Platform, Version};
    use url::Url;

    use crate::{CondaBinaryData, LockFile};

    #[test]
    fn test_merge_records_and_purls() {
        let record = PackageRecord {
            subdir: "linux-64".into(),
            ..PackageRecord::new(
                PackageName::new_unchecked("foobar"),
                Version::from_str("1.0.0").unwrap(),
                "build".into(),
            )
        };

        let record_with_purls = PackageRecord {
            purls: Some(
                ["pkg:pypi/foobar@1.0.0".parse().unwrap()]
                    .into_iter()
                    .collect(),
            ),
            ..record.clone()
        };

        let lock_file = LockFile::builder()
            .with_conda_package(
                "default",
                Platform::Linux64,
                CondaBinaryData {
                    package_record: record.clone(),
                    location: Url::parse(
                        "https://prefix.dev/example/linux-64/foobar-1.0.0-build.tar.bz2",
                    )
                    .unwrap()
                    .into(),
                    file_name: "foobar-1.0.0-build.tar.bz2".to_string(),
                    channel: None,
                }
                .into(),
            )
            .with_conda_package(
                "default",
                Platform::Linux64,
                CondaBinaryData {
                    package_record: record.clone(),
                    location: Url::parse(
                        "https://prefix.dev/example/linux-64/foobar-1.0.0-build.tar.bz2",
                    )
                    .unwrap()
                    .into(),
                    file_name: "foobar-1.0.0-build.tar.bz2".to_string(),
                    channel: None,
                }
                .into(),
            )
            .with_conda_package(
                "foobar",
                Platform::Linux64,
                CondaBinaryData {
                    package_record: record_with_purls,
                    location: Url::parse(
                        "https://prefix.dev/example/linux-64/foobar-1.0.0-build.tar.bz2",
                    )
                    .unwrap()
                    .into(),
                    file_name: "foobar-1.0.0-build.tar.bz2".to_string(),
                    channel: None,
                }
                .into(),
            )
            .finish();
        insta::assert_snapshot!(lock_file.render_to_string().unwrap());
    }
}
