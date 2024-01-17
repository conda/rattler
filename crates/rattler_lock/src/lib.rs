#![deny(missing_docs, dead_code)]

//! Definitions for a lock-file format that stores information about pinned dependencies from both
//! the Conda and Pypi ecosystem.
//!
//! ## Design goals
//!
//! The goal of the lock-file format is:
//!
//! * To be complete. The lock-file should contain all the information needed to recreate
//!   environments even years after it was created. As long as the package data persists that a
//!   lock-file refers to, it should be possible to recreate the environment.
//! * To be human readable. Although lock-files are not intended to be edited by hand, they should
//!   be relatively easy to read and understand. So that when a lock-file is checked into version
//!   control and someone looks at the diff, they can understand what changed.
//! * To be easily parsable. It should be fairly straightforward to create a parser for the format
//!   so that it can be used in other tools.
//! * To reduce diff size when the content changes. The order of content in the serialized lock-file
//!   should be fixed to ensure that the diff size is minimized when the content changes.
//! * To be reproducible. Recreating the lock-file with the exact same input (including externally
//!   fetched data) should yield the same lock-file byte-for-byte.
//! * To be statically verifiable. Given the specifications of the packages that went into a
//!   lock-file it should be possible to cheaply verify whether or not the specifications are still
//!   satisfied by the packages stored in the lock-file.
//! * Forward compatible. Older version of lock-files should still be readable by never versions of
//!   this crate.
//!
//! ## Relation to conda-lock
//!
//! Initially the lock-file format was based on [`conda-lock`](https://github.com/conda/conda-lock)
//! but over time significant changes have been made compared to the original conda-lock format.
//! Conda-lock files (e.g. `conda-lock.yml` files) can still be parsed by this crate but the
//! serialization format changed significantly. This means files created by this crate are not
//! compatible with conda-lock.
//!
//! Conda-lock stores a lot of metadata to be able to verify if the lock-file is still valid given
//! the sources/inputs. For example conda-lock contains a `content-hash` which is a hash of all the
//! input data of the lock-file.
//! This crate approaches this differently by storing enough information in the lock-file to be able
//! to verify if the lock-file still satisfies an input/source without requiring additional input
//! (e.g. network requests) or expensive solves. We call this static satisfiability verification.
//!
//! Conda-lock stores a custom __partial__ representation of a [`rattler_conda_types::RepoDataRecord`]
//! in the lock-file. This poses a problem when incrementally updating an environment. To only
//! partially update packages in the lock-file without completely recreating it, the records stored
//! in the lock-file need to be passed to the solver as "preferred" packages. Since
//! [`rattler_conda_types::MatchSpec`] can match on any field present in a
//! [`rattler_conda_types::PackageRecord`] we need to store all fields in the lock-file not just a
//! subset.
//! To that end this crate stores the full [`rattler_conda_types::PackageRecord`] in the lock-file.
//! This allows completely recreating the record that was read from repodata when the lock-file was
//! created which will allow a correct incremental update.
//!
//! Conda-lock requires users to create multiple lock-files when they want to store multiple
//! environments. This crate allows storing multiple environments for different platforms and with
//! different channels in a single lock-file. This allows storing production- and test environments
//! in a single file.

use fxhash::FxHashMap;
use rattler_conda_types::Platform;
use std::{borrow::Cow, io::Read, path::Path, str::FromStr};
use url::Url;

pub mod builder;
mod channel;
mod hash;
mod package;
mod parse;
mod utils;

pub use channel::Channel;

use crate::package::{CondaPackageData, PypiPackageData, RuntimePackageData};

use crate::builder::LockFileBuilder;
pub use hash::PackageHashes;
pub use package::PyPiRuntimeConfiguration;

pub use self::parse::ParseCondaLockError;

/// The name of the default environment in a [`LockFile`]. This is the environment name that is used
/// when no explicit environment name is specified.
pub const DEFAULT_ENVIRONMENT_NAME: &str = "default";

/// Represents a lock-file for both Conda packages and Pypi packages.
///
/// Lock-files can store information for multiple platforms and for multiple environments.
#[derive(Clone, Debug)]
pub struct LockFile {
    /// Metadata about the different environments stored in the lock file.
    environments: FxHashMap<String, EnvironmentData>,

    conda_packages: Vec<CondaPackageData>,
    pypi_packages: Vec<PypiPackageData>,
}

/// Information about a specific environment in the lock file.
///
/// This only needs to store information about an environment that cannot be derived from the
/// packages itself.
///
/// The default environment is called "default".
#[derive(Clone, Debug)]
struct EnvironmentData {
    /// The channels used to solve the environment. Note that the order matters.
    channels: Vec<Channel>,

    /// For each individual platform this environment supports we store the package identifiers
    /// associated with the environment.
    packages: FxHashMap<Platform, Vec<RuntimePackageData>>,
}

impl LockFile {
    /// Constructs a new lock-file builder. This is the preferred way to constructs a lock-file
    /// programmatically.
    pub fn builder() -> LockFileBuilder {
        LockFileBuilder::new()
    }

    /// Parses an conda-lock file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<Self, ParseCondaLockError> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses an conda-lock file from a file.
    pub fn from_path(path: &Path) -> Result<Self, ParseCondaLockError> {
        let source = std::fs::read_to_string(path)?;
        Self::from_str(&source)
    }

    /// Writes the conda lock to a file
    pub fn to_path(&self, path: &Path) -> Result<(), std::io::Error> {
        let file = std::fs::File::create(path)?;
        serde_yaml::to_writer(file, self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
    }

    /// Returns the environment with the given name.
    pub fn environment(&self, name: &str) -> Option<Environment<'_>> {
        self.environments.get(name).map(|env| Environment {
            lock_file: self,
            environment: env,
        })
    }

    /// Returns an iterator over all environments defined in the lock-file.
    pub fn environments(
        &self,
    ) -> impl Iterator<Item = (&str, Environment<'_>)> + ExactSizeIterator + '_ {
        self.environments.iter().map(move |(name, env)| {
            (
                name.as_str(),
                Environment {
                    lock_file: self,
                    environment: env,
                },
            )
        })
    }
}

/// Information about a specific environment in the lock-file.
///
/// The `'l` lifetime parameter refers to the lifetime of the lock file in which the data is stored.
#[derive(Copy, Clone)]
pub struct Environment<'l> {
    lock_file: &'l LockFile,
    environment: &'l EnvironmentData,
}

impl<'l> Environment<'l> {
    /// Returns all the platforms for which we have a locked-down environment.
    pub fn platforms(&self) -> impl Iterator<Item = Platform> + ExactSizeIterator + '_ {
        self.environment.packages.keys().copied()
    }

    /// Returns the channels that are used by this environment.
    ///
    /// Note that the order of the channels is significant. The first channel is the highest
    /// priority channel.
    pub fn channels(&self) -> &'l [Channel] {
        &self.environment.channels
    }

    /// Returns all the packages for a specific platform in this environment.
    pub fn packages(
        &self,
        platform: Platform,
    ) -> Option<impl Iterator<Item = Package<'l>> + ExactSizeIterator + '_> {
        let packages = self.environment.packages.get(&platform)?;
        Some(packages.iter().map(move |package| match package {
            RuntimePackageData::Conda(idx) => Package::Conda(CondaPackage {
                package: &self.lock_file.conda_packages[*idx],
            }),
            RuntimePackageData::Pypi(idx, runtime) => Package::Pypi(PypiPackage {
                package: &self.lock_file.pypi_packages[*idx],
                runtime,
            }),
        }))
    }
}

/// Data related to a single locked package in an [`Environment`].
///
/// The `'l` lifetime parameter refers to the lifetime of the lock file in which the data is stored.
#[derive(Copy, Clone)]
pub enum Package<'l> {
    /// A conda package
    Conda(CondaPackage<'l>),

    /// A pypi package
    Pypi(PypiPackage<'l>),
}

impl<'l> Package<'l> {
    /// Returns true if this package represents a conda package.
    pub fn is_conda(&self) -> bool {
        matches!(self, Self::Conda(_))
    }

    /// Returns true if this package represents a pypi package.
    pub fn is_pypi(&self) -> bool {
        matches!(self, Self::Pypi(_))
    }

    /// Returns this instance as a [`CondaPackage`] if this instance represents a conda
    /// package.
    pub fn as_conda(&self) -> Option<CondaPackage<'l>> {
        match self {
            Self::Conda(value) => Some(*value),
            Self::Pypi(_) => None,
        }
    }

    /// Returns this instance as a [`PypiPackage`] if this instance represents a pypi
    /// package.
    pub fn as_pypi(&self) -> Option<PypiPackage<'l>> {
        match self {
            Self::Conda(_) => None,
            Self::Pypi(value) => Some(*value),
        }
    }

    /// Returns the name of the package.
    pub fn name(&self) -> &'l str {
        match self {
            Self::Conda(value) => value.package.package_record.name.as_normalized(),
            Self::Pypi(value) => value.package.name.as_str(),
        }
    }

    /// Returns the version string of the package
    pub fn version(&self) -> Cow<'l, str> {
        match self {
            Self::Conda(value) => value.package.package_record.version.as_str(),
            Self::Pypi(value) => value.package.version.to_string().into(),
        }
    }

    /// Returns the URL of the package
    pub fn url(&self) -> &'l Url {
        match self {
            Package::Conda(value) => &value.package.url,
            Package::Pypi(value) => &value.package.url,
        }
    }
}

/// Data related to a single locked conda package in an environment.
///
/// The `'l` lifetime parameter refers to the lifetime of the lock file in which the data is stored.
#[derive(Copy, Clone)]
pub struct CondaPackage<'l> {
    /// The package data
    pub package: &'l CondaPackageData,
}

/// Data related to a single locked pypi package in an environment.
///
/// The `'l` lifetime parameter refers to the lifetime of the lock file in which the data is stored.
#[derive(Copy, Clone)]
pub struct PypiPackage<'l> {
    /// The package data
    pub package: &'l PypiPackageData,

    /// Additional information for the package that is specific to an environment/platform. E.g.
    /// different environments might refer to the same pypi package but with different extras
    /// enabled.
    pub runtime: &'l PyPiRuntimeConfiguration,
}

#[cfg(test)]
mod test {
    use super::{LockFile, DEFAULT_ENVIRONMENT_NAME};
    use rattler_conda_types::Platform;
    use rstest::*;
    use std::path::Path;

    #[rstest]
    #[case("v0/numpy-conda-lock.yml")]
    #[case("v0/python-conda-lock.yml")]
    #[case("v0/pypi-matplotlib-conda-lock.yml")]
    #[case("v3/robostack-turtlesim-conda-lock.yml")]
    #[case("v4/numpy-lock.yml")]
    #[case("v4/python-lock.yml")]
    #[case("v4/pypi-matplotlib-lock.yml")]
    #[case("v4/turtlesim-lock.yml")]
    fn test_parse(#[case] file_name: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/conda-lock")
            .join(file_name);
        let conda_lock = LockFile::from_path(&path).unwrap();
        insta::assert_yaml_snapshot!(file_name, conda_lock);
    }

    #[test]
    fn packages_for_platform() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/conda-lock")
            .join("v0/numpy-conda-lock.yml");

        // Try to read conda_lock
        let conda_lock = LockFile::from_path(&path).unwrap();

        insta::assert_yaml_snapshot!(conda_lock
            .environment(DEFAULT_ENVIRONMENT_NAME)
            .unwrap()
            .packages(Platform::Linux64)
            .unwrap()
            .map(|p| p.url())
            .collect::<Vec<_>>());

        insta::assert_yaml_snapshot!(conda_lock
            .environment(DEFAULT_ENVIRONMENT_NAME)
            .unwrap()
            .packages(Platform::Osx64)
            .unwrap()
            .map(|p| p.url())
            .collect::<Vec<_>>());
    }
}
