//! Builder for the creation of lock files.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use fxhash::FxHashMap;
use indexmap::{IndexMap, IndexSet};
use pep508_rs::ExtraName;
use rattler_conda_types::Platform;

use crate::{
    file_format_version::FileFormatVersion, Channel, CondaPackageData, EnvironmentData,
    EnvironmentPackageData, LockFile, LockFileInner, Package, PypiIndexes, PypiPackageData,
    PypiPackageEnvironmentData,
};

/// A struct to incrementally build a lock-file.
#[derive(Default)]
pub struct LockFileBuilder {
    /// Metadata about the different environments stored in the lock file.
    environments: IndexMap<String, EnvironmentData>,

    /// A list of all package metadata stored in the lock file.
    conda_packages: IndexSet<CondaPackageData>,
    pypi_packages: IndexSet<PypiPackageData>,
    pypi_runtime_configurations: IndexSet<HashablePypiPackageEnvironmentData>,
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

        // Add the package to the list of packages.
        let package_idx = self.conda_packages.insert_full(locked_package).0;

        // Add the package to the environment that it is intended for.
        environment
            .packages
            .entry(platform)
            .or_default()
            .push(EnvironmentPackageData::Conda(package_idx));

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
            .push(EnvironmentPackageData::Pypi(package_idx, runtime_idx));

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
        locked_package: Package,
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
        locked_package: Package,
    ) -> &mut Self {
        match locked_package {
            Package::Conda(p) => {
                self.add_conda_package(environment, platform, p.package_data().clone())
            }
            Package::Pypi(p) => self.add_pypi_package(
                environment,
                platform,
                p.package_data().clone(),
                p.environment_data().clone(),
            ),
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
                conda_packages: self.conda_packages.into_iter().collect(),
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
