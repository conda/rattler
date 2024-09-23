#![deny(missing_docs, dead_code)]

//! Definitions for a lock-file format that stores information about pinned
//! dependencies from both the Conda and Pypi ecosystem.
//!
//! The crate is structured in two API levels.
//!
//! 1. The top level API accessible through the [`LockFile`] type that exposes
//!    high level access to the lock-file. This API is intended to be relatively
//!    stable and is the preferred way to interact with the lock-file.
//! 2. The `*Data` types. These are lower level types that expose more of the
//!    internal data structures used in the crate. These types are not intended
//!    to be stable and are subject to change over time. These types are used
//!    internally by the top level API. Also note that only a subset of the
//!    `*Data` types are exposed. See `[crate::PyPiPackageData]`,
//!    `[crate::CondaPackageData]` for examples.
//!
//! ## Design goals
//!
//! The goal of the lock-file format is:
//!
//! * To be complete. The lock-file should contain all the information needed to
//!   recreate environments even years after it was created. As long as the
//!   package data persists that a lock-file refers to, it should be possible to
//!   recreate the environment.
//! * To be human readable. Although lock-files are not intended to be edited by
//!   hand, they should be relatively easy to read and understand. So that when
//!   a lock-file is checked into version control and someone looks at the diff,
//!   they can understand what changed.
//! * To be easily parsable. It should be fairly straightforward to create a
//!   parser for the format so that it can be used in other tools.
//! * To reduce diff size when the content changes. The order of content in the
//!   serialized lock-file should be fixed to ensure that the diff size is
//!   minimized when the content changes.
//! * To be reproducible. Recreating the lock-file with the exact same input
//!   (including externally fetched data) should yield the same lock-file
//!   byte-for-byte.
//! * To be statically verifiable. Given the specifications of the packages that
//!   went into a lock-file it should be possible to cheaply verify whether or
//!   not the specifications are still satisfied by the packages stored in the
//!   lock-file.
//! * Backward compatible. Older version of lock-files should still be readable
//!   by never versions of this crate.
//!
//! ## Relation to conda-lock
//!
//! Initially the lock-file format was based on [`conda-lock`](https://github.com/conda/conda-lock)
//! but over time significant changes have been made compared to the original
//! conda-lock format. Conda-lock files (e.g. `conda-lock.yml` files) can still
//! be parsed by this crate but the serialization format changed significantly.
//! This means files created by this crate are not compatible with conda-lock.
//!
//! Conda-lock stores a lot of metadata to be able to verify if the lock-file is
//! still valid given the sources/inputs. For example conda-lock contains a
//! `content-hash` which is a hash of all the input data of the lock-file.
//! This crate approaches this differently by storing enough information in the
//! lock-file to be able to verify if the lock-file still satisfies an
//! input/source without requiring additional input (e.g. network requests) or
//! expensive solves. We call this static satisfiability verification.
//!
//! Conda-lock stores a custom __partial__ representation of a
//! [`rattler_conda_types::RepoDataRecord`] in the lock-file. This poses a
//! problem when incrementally updating an environment. To only partially update
//! packages in the lock-file without completely recreating it, the records
//! stored in the lock-file need to be passed to the solver as "preferred"
//! packages. Since [`rattler_conda_types::MatchSpec`] can match on any field
//! present in a [`rattler_conda_types::PackageRecord`] we need to store all
//! fields in the lock-file not just a subset.
//! To that end this crate stores the full
//! [`rattler_conda_types::PackageRecord`] in the lock-file. This allows
//! completely recreating the record that was read from repodata when the
//! lock-file was created which will allow a correct incremental update.
//!
//! Conda-lock requires users to create multiple lock-files when they want to
//! store multiple environments. This crate allows storing multiple environments
//! for different platforms and with different channels in a single lock-file.
//! This allows storing production- and test environments in a single file.

use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    io::Read,
    path::Path,
    str::FromStr,
    sync::Arc,
};

use fxhash::FxHashMap;
use pep508_rs::{ExtraName, Requirement};
use rattler_conda_types::{MatchSpec, PackageRecord, Platform, RepoDataRecord};
use url::Url;

mod builder;
mod channel;
mod conda;
mod file_format_version;
mod hash;
mod parse;
mod pypi;
mod pypi_indexes;
mod url_or_path;
mod utils;

pub use builder::LockFileBuilder;
pub use channel::Channel;
pub use conda::{CondaPackageData, ConversionError, InputHash};
pub use file_format_version::FileFormatVersion;
pub use hash::PackageHashes;
pub use parse::ParseCondaLockError;
pub use pypi::{PypiPackageData, PypiPackageEnvironmentData, PypiSourceTreeHashable};
pub use pypi_indexes::{FindLinksUrlOrPath, PypiIndexes};
pub use rattler_conda_types::Matches;
pub use url_or_path::UrlOrPath;

/// The name of the default environment in a [`LockFile`]. This is the
/// environment name that is used when no explicit environment name is
/// specified.
pub const DEFAULT_ENVIRONMENT_NAME: &str = "default";

/// Represents a lock-file for both Conda packages and Pypi packages.
///
/// Lock-files can store information for multiple platforms and for multiple
/// environments.
///
/// The high-level API provided by this type holds internal references to the
/// data. Its is therefore cheap to clone this type and any type derived from it
/// (e.g. [`Environment`] or [`Package`]).
#[derive(Clone, Default)]
pub struct LockFile {
    inner: Arc<LockFileInner>,
}

/// Internal data structure that stores the lock-file data.
#[derive(Default)]
struct LockFileInner {
    version: FileFormatVersion,
    environments: Vec<EnvironmentData>,
    conda_packages: Vec<CondaPackageData>,
    pypi_packages: Vec<PypiPackageData>,
    pypi_environment_package_data: Vec<PypiPackageEnvironmentData>,

    environment_lookup: FxHashMap<String, usize>,
}

/// An package used in an environment. Selects a type of package based on the
/// enum and might contain additional data that is specific to the environment.
/// For instance different environments might select the same Pypi package but
/// with different extras.
#[derive(Clone, Copy, Debug)]
enum EnvironmentPackageData {
    Conda(usize),
    Pypi(usize, usize),
}

/// Information about a specific environment in the lock file.
///
/// This only needs to store information about an environment that cannot be
/// derived from the packages itself.
///
/// The default environment is called "default".
#[derive(Clone, Debug)]
struct EnvironmentData {
    /// The channels used to solve the environment. Note that the order matters.
    channels: Vec<Channel>,

    /// The pypi indexes used to solve the environment.
    indexes: Option<PypiIndexes>,

    /// For each individual platform this environment supports we store the
    /// package identifiers associated with the environment.
    packages: FxHashMap<Platform, Vec<EnvironmentPackageData>>,
}

impl LockFile {
    /// Constructs a new lock-file builder. This is the preferred way to
    /// constructs a lock-file programmatically.
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

    /// Writes the conda lock to a string
    pub fn render_to_string(&self) -> Result<String, std::io::Error> {
        serde_yaml::to_string(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
    }

    /// Returns the environment with the given name.
    pub fn environment(&self, name: &str) -> Option<Environment> {
        let index = *self.inner.environment_lookup.get(name)?;
        Some(Environment {
            inner: self.inner.clone(),
            index,
        })
    }

    /// Returns the environment with the default name as defined by
    /// [`DEFAULT_ENVIRONMENT_NAME`].
    pub fn default_environment(&self) -> Option<Environment> {
        self.environment(DEFAULT_ENVIRONMENT_NAME)
    }

    /// Returns an iterator over all environments defined in the lock-file.
    pub fn environments(&self) -> impl ExactSizeIterator<Item = (&str, Environment)> + '_ {
        self.inner
            .environment_lookup
            .iter()
            .map(move |(name, index)| {
                (
                    name.as_str(),
                    Environment {
                        inner: self.inner.clone(),
                        index: *index,
                    },
                )
            })
    }

    /// Returns the version of the lock-file.
    pub fn version(&self) -> FileFormatVersion {
        self.inner.version
    }
}

/// Information about a specific environment in the lock-file.
#[derive(Clone)]
pub struct Environment {
    inner: Arc<LockFileInner>,
    index: usize,
}

impl Environment {
    /// Returns a reference to the internal data structure.
    fn data(&self) -> &EnvironmentData {
        &self.inner.environments[self.index]
    }

    /// Returns all the platforms for which we have a locked-down environment.
    pub fn platforms(&self) -> impl ExactSizeIterator<Item = Platform> + '_ {
        self.data().packages.keys().copied()
    }

    /// Returns the channels that are used by this environment.
    ///
    /// Note that the order of the channels is significant. The first channel is
    /// the highest priority channel.
    pub fn channels(&self) -> &[Channel] {
        &self.data().channels
    }

    /// Returns the Pypi indexes that were used to solve this environment.
    ///
    /// If there are no pypi packages in the lock-file this will return `None`.
    ///
    /// Starting with version `5` of the format this should not be optional.
    pub fn pypi_indexes(&self) -> Option<&PypiIndexes> {
        self.data().indexes.as_ref()
    }

    /// Returns all the packages for a specific platform in this environment.
    pub fn packages(
        &self,
        platform: Platform,
    ) -> Option<impl DoubleEndedIterator<Item = Package> + ExactSizeIterator + '_> {
        Some(
            self.data()
                .packages
                .get(&platform)?
                .iter()
                .map(move |package| Package::from_env_package(*package, self.inner.clone())),
        )
    }

    /// Returns an iterator over all packages and platforms defined for this
    /// environment
    pub fn packages_by_platform(
        &self,
    ) -> impl ExactSizeIterator<
        Item = (
            Platform,
            impl DoubleEndedIterator<Item = Package> + ExactSizeIterator + '_,
        ),
    > + '_ {
        let env_data = self.data();
        env_data.packages.iter().map(move |(platform, packages)| {
            (
                *platform,
                packages
                    .iter()
                    .map(move |package| Package::from_env_package(*package, self.inner.clone())),
            )
        })
    }

    /// Returns all pypi packages for all platforms
    pub fn pypi_packages(
        &self,
    ) -> impl ExactSizeIterator<
        Item = (
            Platform,
            impl DoubleEndedIterator<Item = (&PypiPackageData, &PypiPackageEnvironmentData)>,
        ),
    > + '_ {
        let env_data = self.data();
        env_data.packages.iter().map(|(platform, packages)| {
            let records = packages.iter().filter_map(|package| match package {
                EnvironmentPackageData::Conda(_) => None,
                EnvironmentPackageData::Pypi(pkg_data_idx, env_data_idx) => Some((
                    &self.inner.pypi_packages[*pkg_data_idx],
                    &self.inner.pypi_environment_package_data[*env_data_idx],
                )),
            });
            (*platform, records)
        })
    }

    /// Returns all conda packages for all platforms.
    pub fn conda_packages(
        &self,
    ) -> impl ExactSizeIterator<Item = (Platform, impl DoubleEndedIterator<Item = &CondaPackageData>)> + '_
    {
        self.data().packages.iter().map(|(platform, packages)| {
            (
                *platform,
                packages.iter().filter_map(|package| match package {
                    EnvironmentPackageData::Conda(idx) => Some(&self.inner.conda_packages[*idx]),
                    EnvironmentPackageData::Pypi(_, _) => None,
                }),
            )
        })
    }

    /// Returns all conda packages for all platforms and converts them to
    /// [`RepoDataRecord`].
    pub fn conda_repodata_records(
        &self,
    ) -> Result<HashMap<Platform, Vec<RepoDataRecord>>, ConversionError> {
        self.conda_packages()
            .map(|(platform, packages)| {
                Ok((
                    platform,
                    packages
                        .map(RepoDataRecord::try_from)
                        .collect::<Result<Vec<_>, ConversionError>>()?,
                ))
            })
            .collect()
    }

    /// Returns all conda packages for a specific platform.
    pub fn conda_packages_for_platform(
        &self,
        platform: Platform,
    ) -> Option<impl DoubleEndedIterator<Item = &CondaPackageData> + '_> {
        Some(
            self.data()
                .packages
                .get(&platform)?
                .iter()
                .filter_map(|package| match package {
                    EnvironmentPackageData::Conda(idx) => Some(&self.inner.conda_packages[*idx]),
                    EnvironmentPackageData::Pypi(_, _) => None,
                }),
        )
    }

    /// Takes all the conda packages, converts them to [`RepoDataRecord`] and
    /// returns them or returns an error if the conversion failed. Returns
    /// `None` if the specified platform is not defined for this
    /// environment.
    ///
    /// This method ignores any conda packages that do not refer to repodata
    /// records.
    pub fn conda_repodata_records_for_platform(
        &self,
        platform: Platform,
    ) -> Result<Option<Vec<RepoDataRecord>>, ConversionError> {
        self.conda_packages_for_platform(platform)
            .map(|packages| packages.map(RepoDataRecord::try_from).collect())
            .transpose()
    }

    /// Returns all the pypi packages and their associated environment data for
    /// the specified platform. Returns `None` if the platform is not
    /// defined for this environment.
    pub fn pypi_packages_for_platform(
        &self,
        platform: Platform,
    ) -> Option<impl DoubleEndedIterator<Item = (&PypiPackageData, &PypiPackageEnvironmentData)> + '_>
    {
        Some(
            self.data()
                .packages
                .get(&platform)?
                .iter()
                .filter_map(|package| match package {
                    EnvironmentPackageData::Conda(_) => None,
                    EnvironmentPackageData::Pypi(package_idx, env_idx) => Some((
                        &self.inner.pypi_packages[*package_idx],
                        &self.inner.pypi_environment_package_data[*env_idx],
                    )),
                }),
        )
    }

    /// Returns the version of the lock-file that contained this environment.
    pub fn version(&self) -> FileFormatVersion {
        self.inner.version
    }
}

/// Data related to a single locked package in an [`Environment`].
#[derive(Clone)]
pub enum Package {
    /// A conda package
    Conda(CondaPackage),

    /// A pypi package
    Pypi(PypiPackage),
}

impl From<CondaPackage> for Package {
    fn from(value: CondaPackage) -> Self {
        Package::Conda(value)
    }
}

impl From<PypiPackage> for Package {
    fn from(value: PypiPackage) -> Self {
        Package::Pypi(value)
    }
}

impl Package {
    /// Constructs a new instance from a [`EnvironmentPackageData`] and a
    /// reference to the internal data structure.
    fn from_env_package(data: EnvironmentPackageData, inner: Arc<LockFileInner>) -> Self {
        match data {
            EnvironmentPackageData::Conda(idx) => {
                Package::Conda(CondaPackage { inner, index: idx })
            }
            EnvironmentPackageData::Pypi(idx, runtime) => Package::Pypi(PypiPackage {
                inner,
                package_index: idx,
                runtime_index: runtime,
            }),
        }
    }

    /// Returns true if this package represents a conda package.
    pub fn is_conda(&self) -> bool {
        matches!(self, Self::Conda(_))
    }

    /// Returns true if this package represents a pypi package.
    pub fn is_pypi(&self) -> bool {
        matches!(self, Self::Pypi(_))
    }

    /// Returns this instance as a [`CondaPackage`] if this instance represents
    /// a conda package.
    pub fn as_conda(&self) -> Option<&CondaPackage> {
        match self {
            Self::Conda(value) => Some(value),
            Self::Pypi(_) => None,
        }
    }

    /// Returns this instance as a [`PypiPackage`] if this instance represents a
    /// pypi package.
    pub fn as_pypi(&self) -> Option<&PypiPackage> {
        match self {
            Self::Conda(_) => None,
            Self::Pypi(value) => Some(value),
        }
    }

    /// Returns this instance as a [`CondaPackage`] if this instance represents
    /// a conda package.
    pub fn into_conda(self) -> Option<CondaPackage> {
        match self {
            Self::Conda(value) => Some(value),
            Self::Pypi(_) => None,
        }
    }

    /// Returns this instance as a [`PypiPackage`] if this instance represents a
    /// pypi package.
    pub fn into_pypi(self) -> Option<PypiPackage> {
        match self {
            Self::Conda(_) => None,
            Self::Pypi(value) => Some(value),
        }
    }

    /// Returns the name of the package.
    pub fn name(&self) -> Cow<'_, str> {
        match self {
            Self::Conda(value) => value.package_record().name.as_normalized().into(),
            Self::Pypi(value) => value.package_data().name.as_dist_info_name(),
        }
    }

    /// Returns the version string of the package
    pub fn version(&self) -> Cow<'_, str> {
        match self {
            Self::Conda(value) => value.package_record().version.as_str(),
            Self::Pypi(value) => value.package_data().version.to_string().into(),
        }
    }

    /// Returns the URL or relative path to the package
    pub fn location(&self) -> &UrlOrPath {
        match self {
            Self::Conda(value) => value.location(),
            Self::Pypi(value) => value.location(),
        }
    }
}

/// Data related to a single locked conda package in an environment.
#[derive(Clone)]
pub struct CondaPackage {
    inner: Arc<LockFileInner>,
    index: usize,
}

impl CondaPackage {
    /// Returns the package data
    pub fn package_data(&self) -> &CondaPackageData {
        &self.inner.conda_packages[self.index]
    }

    /// Returns the package data
    pub fn package_record(&self) -> &PackageRecord {
        &self.package_data().package_record
    }

    /// Returns the location of the package, this is the place where the package
    /// can be downloaded from.
    pub fn location(&self) -> &UrlOrPath {
        &self.package_data().location
    }

    /// Returns the channel of the package.
    pub fn channel(&self) -> Option<Url> {
        self.package_data().channel.clone()
    }

    /// Returns true if this package satisfies the given `spec`.
    pub fn satisfies(&self, spec: &MatchSpec) -> bool {
        // Check the data in the package record
        if !spec.matches(self.package_record()) {
            return false;
        }

        // Check the the channel
        if let Some(channel) = &spec.channel {
            if !self
                .location()
                .as_str()
                .starts_with(channel.base_url.as_str())
            {
                return false;
            }
        }

        true
    }
}

impl AsRef<PackageRecord> for CondaPackage {
    fn as_ref(&self) -> &PackageRecord {
        self.package_record()
    }
}

impl TryFrom<CondaPackage> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: CondaPackage) -> Result<Self, Self::Error> {
        value.package_data().clone().try_into()
    }
}

/// Data related to a single locked pypi package in an environment.
#[derive(Clone)]
pub struct PypiPackage {
    inner: Arc<LockFileInner>,
    package_index: usize,
    runtime_index: usize,
}

impl PypiPackage {
    /// Returns the runtime data from the internal data structure.
    pub fn environment_data(&self) -> &PypiPackageEnvironmentData {
        &self.inner.pypi_environment_package_data[self.runtime_index]
    }

    /// Returns the package data from the internal data structure.
    pub fn package_data(&self) -> &PypiPackageData {
        &self.inner.pypi_packages[self.package_index]
    }

    /// Returns the location of the package, this is the place where the package
    /// can be downloaded from.
    pub fn location(&self) -> &UrlOrPath {
        &self.package_data().location
    }

    /// Returns the extras enabled for this package
    pub fn extras(&self) -> &BTreeSet<ExtraName> {
        &self.environment_data().extras
    }

    /// Returns true if this package satisfies the given `spec`.
    pub fn satisfies(&self, spec: &Requirement) -> bool {
        self.package_data().satisfies(spec)
    }

    /// Returns true if this package should be installed in "editable" mode.
    pub fn is_editable(&self) -> bool {
        self.package_data().editable
    }
}

#[cfg(test)]
mod test {
    use std::{
        path::{Path, PathBuf},
        str::FromStr,
    };

    use rattler_conda_types::Platform;
    use rstest::*;

    use super::{LockFile, DEFAULT_ENVIRONMENT_NAME};

    #[rstest]
    #[case::v0_numpy("v0/numpy-conda-lock.yml")]
    #[case::v0_python("v0/python-conda-lock.yml")]
    #[case::v0_pypi_matplotlib("v0/pypi-matplotlib-conda-lock.yml")]
    #[case::v3_robostack("v3/robostack-turtlesim-conda-lock.yml")]
    #[case::v3_numpy("v4/numpy-lock.yml")]
    #[case::v4_python("v4/python-lock.yml")]
    #[case::v4_pypi_matplotlib("v4/pypi-matplotlib-lock.yml")]
    #[case::v4_turtlesim("v4/turtlesim-lock.yml")]
    #[case::v4_pypi_path("v4/path-based-lock.yml")]
    #[case::v4_pypi_absolute_path("v4/absolute-path-lock.yml")]
    #[case::v5_pypi_flat_index("v5/flat-index-lock.yml")]
    #[case::v6_conda_source_path("v6/conda-path-lock.yml")]
    #[case::v6_derived_channel("v6/derived-channel-lock.yml")]
    fn test_parse(#[case] file_name: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/conda-lock")
            .join(file_name);
        let conda_lock = LockFile::from_path(&path).unwrap();
        insta::assert_yaml_snapshot!(file_name, conda_lock);
    }

    #[rstest]
    fn test_roundtrip(
        #[files("../../test-data/conda-lock/**/*.yml")]
        #[exclude("forward-compatible-lock")]
        path: PathBuf,
    ) {
        // Load the lock-file
        let conda_lock = LockFile::from_path(&path).unwrap();

        // Serialize the lock-file
        let rendered_lock_file = conda_lock.render_to_string().unwrap();

        // Parse the rendered lock-file again
        let parsed_lock_file = LockFile::from_str(&rendered_lock_file).unwrap();

        // And re-render again
        let rerendered_lock_file = parsed_lock_file.render_to_string().unwrap();

        similar_asserts::assert_eq!(rendered_lock_file, rerendered_lock_file);
    }

    /// Absolute paths on Windows are not properly parsed.
    /// See: <https://github.com/conda/rattler/issues/615>
    #[test]
    fn test_issue_615() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/conda-lock/v4/absolute-path-lock.yml");
        let conda_lock = LockFile::from_path(&path);
        assert!(conda_lock.is_ok());
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
            .map(|p| p.location().to_string())
            .collect::<Vec<_>>());

        insta::assert_yaml_snapshot!(conda_lock
            .environment(DEFAULT_ENVIRONMENT_NAME)
            .unwrap()
            .packages(Platform::Osx64)
            .unwrap()
            .map(|p| p.location().to_string())
            .collect::<Vec<_>>());
    }
}
