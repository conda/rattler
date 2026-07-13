//! Builder for the creation of lock files.

use std::{borrow::Cow, collections::HashMap, sync::Arc};

use indexmap::IndexMap;
use rattler_conda_types::Version;

use crate::{
    Channel, CondaBinaryData, CondaPackageData, CondaSourceData, EnvironmentData, EnvironmentIndex,
    EnvironmentPackages, InconsistentInsertError, InvalidPackageHandleError, LockFile,
    LockFileInner, PackageHandle, PackageIndex, ParseCondaLockError, PlatformData, PlatformIndex,
    PypiIndexes, PypiPackageData, PypiPrereleaseMode, PypiSourceData, SolveOptions, SourceData,
    SourceIdentifier, UrlOrPath, Verbatim, file_format_version::FileFormatVersion,
};

/// Information about a single locked package in an environment.
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum LockedPackage {
    /// A conda package
    Conda(CondaPackageData),

    /// A pypi package in an environment
    Pypi(PypiPackageData),
}

impl LockedPackage {
    /// Returns the name of the package as it occurs in the lock file. This
    /// might not be the normalized name.
    pub fn name(&self) -> &str {
        match self {
            LockedPackage::Conda(data) => data.name().as_source(),
            LockedPackage::Pypi(data) => data.name().as_ref(),
        }
    }

    /// Returns the location of the package.
    pub fn location(&self) -> &UrlOrPath {
        match self {
            LockedPackage::Conda(data) => data.location(),
            LockedPackage::Pypi(data) => data.location().inner(),
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
    pub fn as_pypi(&self) -> Option<&PypiPackageData> {
        match self {
            LockedPackage::Conda(..) => None,
            LockedPackage::Pypi(data) => Some(data),
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
    pub fn into_pypi(self) -> Option<PypiPackageData> {
        match self {
            LockedPackage::Conda(..) => None,
            LockedPackage::Pypi(data) => Some(data),
        }
    }
}

/// A struct to incrementally build a lock-file.
#[derive(Default)]
pub struct LockFileBuilder {
    /// The known platforms
    platforms: Vec<PlatformData>,

    /// Metadata about the different environments stored in the lock file.
    environments: IndexMap<String, EnvironmentData>,

    /// All packages stored in the lock file.
    packages: Vec<LockedPackage>,

    /// Maps unique binary package identifiers to their index in `conda_packages`.
    /// Used for deduplication of binary packages.
    binary_package_indices: HashMap<UniqueBinaryIdentifier, PackageIndex>,

    /// Maps source identifiers to their index in `conda_packages`.
    /// Used for deduplication of source packages.
    source_package_indices: HashMap<SourceIdentifier, PackageIndex>,

    /// Maps pypi package locations to their index in `pypi_packages`.
    /// Used for deduplication of pypi packages.
    pypi_package_indices: HashMap<Verbatim<UrlOrPath>, PackageIndex>,
}

/// A unique identifier for a binary conda package. This is used to deduplicate
/// packages. This only includes the unique identifying aspects of a package.
#[derive(Debug, Hash, Eq, PartialEq)]
struct UniqueBinaryIdentifier {
    location: UrlOrPath,
    normalized_name: String,
    version: Version,
    build: String,
    subdir: String,
}

impl<'a> From<&'a CondaBinaryData> for UniqueBinaryIdentifier {
    fn from(data: &'a CondaBinaryData) -> Self {
        Self {
            location: data.location.clone(),
            normalized_name: data.package_record.name.as_normalized().to_string(),
            version: data.package_record.version.version().clone(),
            build: data.package_record.build.clone(),
            subdir: data.package_record.subdir.clone(),
        }
    }
}

/// Error returned by [`LockFileBuilder::register_conda_source_package`] and
/// [`LockFileBuilder::register_pypi_source_package`].
#[derive(Debug, thiserror::Error)]
pub enum RegisterSourcePackageError {
    /// A build or host [`PackageHandle`] does not refer to a package
    /// registered with this builder.
    #[error(transparent)]
    InvalidHandle(#[from] InvalidPackageHandleError),

    /// The build or host handle list reuses either a [`PackageIndex`] or a
    /// selector id with a different counterpart.
    #[error(transparent)]
    InconsistentInsert(#[from] InconsistentInsertError),
}

/// Merges `requires_dist` from `other` into `existing`, adding any entries
/// not already present. This handles the case where different environments
/// produce different marker-evaluated dependency lists for the same package.
fn merge_pypi_requires_dist(existing: &mut PypiPackageData, other: &PypiPackageData) {
    let (PypiPackageData::Distribution(existing), PypiPackageData::Distribution(other)) =
        (existing, other)
    else {
        return;
    };
    for req in &other.requires_dist {
        if !existing.requires_dist.contains(req) {
            existing.requires_dist.push(req.clone());
        }
    }
}

impl LockFileBuilder {
    /// Generate a new lock file using the builder pattern
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the `Vec<Platform>` into the `LockFile`, replacing any platforms that were
    /// known before.
    pub fn with_platforms(
        mut self,
        platforms: Vec<PlatformData>,
    ) -> Result<Self, ParseCondaLockError> {
        let mut unique_platforms = ahash::HashSet::default();
        for platform in platforms.iter() {
            if !unique_platforms.insert(platform.name.clone()) {
                return Err(ParseCondaLockError::DuplicatePlatformName(
                    platform.name.to_string(),
                ));
            }
        }

        self.platforms = platforms;
        Ok(self)
    }

    /// Sets the `Vec<Platform>` into the `LockFile`, replacing any platforms that were
    /// known before.
    pub fn add_platform(mut self, platform: PlatformData) -> Result<Self, ParseCondaLockError> {
        if self
            .platforms
            .iter()
            .any(|p| p.name.as_str() == platform.name.as_str())
        {
            return Err(ParseCondaLockError::DuplicatePlatformName(
                platform.name.to_string(),
            ));
        }

        self.platforms.push(platform);
        Ok(self)
    }

    fn find_platform_index(&self, platform_name: &str) -> Result<PlatformIndex, ()> {
        if let Some(platform_index) = self
            .platforms
            .iter()
            .position(|p| p.name.as_str() == platform_name)
        {
            Ok(PlatformIndex(platform_index))
        } else {
            Err(())
        }
    }

    /// Helper function that returns the `EnvironmentData` for the environment with the given name.
    fn environment_data(&mut self, environment_data: impl Into<String>) -> &mut EnvironmentData {
        self.environments
            .entry(environment_data.into())
            .or_insert_with(|| EnvironmentData {
                channels: vec![],
                packages: HashMap::default(),
                indexes: None,
                options: SolveOptions::default(),
            })
    }

    /// Sets the pypi indexes for an environment.
    pub fn set_pypi_indexes(
        &mut self,
        environment: impl Into<String>,
        indexes: PypiIndexes,
    ) -> &mut Self {
        self.environment_data(environment).indexes = Some(indexes);
        self
    }

    /// Sets the options for a particular environment.
    pub fn set_options(
        &mut self,
        environment: impl Into<String>,
        options: SolveOptions,
    ) -> &mut Self {
        self.environment_data(environment).options = options;
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

    /// Sets the metadata for an environment.
    pub fn set_channels(
        &mut self,
        environment: impl Into<String>,
        channels: impl IntoIterator<Item = impl Into<Channel>>,
    ) -> &mut Self {
        self.environment_data(environment).channels =
            channels.into_iter().map(Into::into).collect();
        self
    }

    /// Adds a package from another environment to a specific environment and
    /// platform.
    pub fn with_package(
        mut self,
        environment: impl Into<String>,
        platform_name: &str,
        locked_package: LockedPackage,
    ) -> Result<Self, ParseCondaLockError> {
        self.add_package(environment, platform_name, locked_package)?;
        Ok(self)
    }

    /// Adds a package from another environment to a specific environment and
    /// platform.
    pub fn add_package(
        &mut self,
        environment: impl Into<String>,
        platform_name: &str,
        locked_package: LockedPackage,
    ) -> Result<&mut Self, ParseCondaLockError> {
        match locked_package {
            LockedPackage::Conda(p) => {
                self.add_conda_package(environment, platform_name, p)?;
            }
            LockedPackage::Pypi(data) => {
                self.add_pypi_package(environment, platform_name, data)?;
            }
        }
        Ok(self)
    }

    /// Adds a conda locked package to a specific environment and platform.
    ///
    /// This function is similar to [`Self::add_conda_package`] but differs in
    /// that it consumes `self` instead of taking a mutable reference. This
    /// allows for a better interface when modifying an existing instance.
    pub fn with_conda_package(
        mut self,
        environment: impl Into<String>,
        platform_name: &str,
        locked_package: CondaPackageData,
    ) -> Result<Self, ParseCondaLockError> {
        self.add_conda_package(environment, platform_name, locked_package)?;
        Ok(self)
    }

    /// Adds a conda locked package to a specific environment and platform.
    ///
    /// This function is similar to [`Self::with_conda_package`] but differs in
    /// that it takes a mutable reference to self instead of consuming it.
    /// This allows for a more fluent with chaining calls.
    pub fn add_conda_package(
        &mut self,
        environment: impl Into<String>,
        platform_name: &str,
        locked_package: CondaPackageData,
    ) -> Result<&mut Self, ParseCondaLockError> {
        let environment = environment.into();
        let platform_index = self.find_platform_index(platform_name).map_err(|_e| {
            ParseCondaLockError::UnknownPlatform {
                environment: environment.clone(),
                platform: platform_name.to_string(),
            }
        })?;
        let handle = self.register_conda_package(locked_package);

        self.environment_data(environment)
            .packages
            .entry(platform_index)
            .or_default()
            .insert(handle)?;

        Ok(self)
    }

    /// Registers a conda package into the lockfile's package list (with
    /// deduplication/merging) without adding it to any environment. Returns
    /// a [`PackageHandle`] that can be inserted into an
    /// [`EnvironmentPackages`] set later.
    pub fn register_conda_package(&mut self, locked_package: CondaPackageData) -> PackageHandle {
        let package_index = match &locked_package {
            CondaPackageData::Binary(binary_data) => {
                let unique_identifier = UniqueBinaryIdentifier::from(binary_data.as_ref());

                // Check if we already have this binary package
                if let Some(&existing_idx) = self.binary_package_indices.get(&unique_identifier) {
                    // Merge with existing package
                    if let LockedPackage::Conda(CondaPackageData::Binary(existing)) =
                        &mut self.packages[existing_idx.0]
                        && let Cow::Owned(merged) = existing.merge(binary_data.as_ref())
                    {
                        **existing = merged;
                    }
                    existing_idx
                } else {
                    // Add new binary package
                    let index = PackageIndex(self.packages.len());
                    self.packages.push(LockedPackage::Conda(locked_package));
                    self.binary_package_indices.insert(unique_identifier, index);
                    index
                }
            }
            CondaPackageData::Source(source_data) => {
                let identifier = SourceIdentifier::from_source_data(source_data);
                if let Some(&existing_idx) = self.source_package_indices.get(&identifier) {
                    existing_idx
                } else {
                    let index = PackageIndex(self.packages.len());
                    self.source_package_indices.insert(identifier, index);
                    self.packages.push(LockedPackage::Conda(locked_package));
                    index
                }
            }
        };

        PackageHandle::new(package_index, &self.packages[package_index.0])
    }

    /// Adds a pypi locked package to a specific environment and platform.
    ///
    /// This function is similar to [`Self::add_pypi_package`] but differs in
    /// that it consumes `self` instead of taking a mutable reference. This
    /// allows for a better interface when modifying an existing instance.
    pub fn with_pypi_package(
        mut self,
        environment: impl Into<String>,
        platform_name: &str,
        locked_package: PypiPackageData,
    ) -> Result<Self, ParseCondaLockError> {
        self.add_pypi_package(environment, platform_name, locked_package)?;
        Ok(self)
    }

    /// Adds a pypi locked package to a specific environment and platform.
    ///
    /// This function is similar to [`Self::with_pypi_package`] but differs in
    /// that it takes a mutable reference to self instead of consuming it.
    /// This allows for a more fluent with chaining calls.
    pub fn add_pypi_package(
        &mut self,
        environment: impl Into<String>,
        platform_name: &str,
        locked_package: PypiPackageData,
    ) -> Result<&mut Self, ParseCondaLockError> {
        let environment = environment.into();
        let platform_index = self.find_platform_index(platform_name).map_err(|_e| {
            ParseCondaLockError::UnknownPlatform {
                environment: environment.clone(),
                platform: platform_name.to_string(),
            }
        })?;
        let handle = self.register_pypi_package(locked_package);

        self.environment_data(environment)
            .packages
            .entry(platform_index)
            .or_default()
            .insert(handle)?;

        Ok(self)
    }

    /// Registers a pypi package into the lockfile's package list
    /// (deduplicating by location and merging `requires_dist`) without
    /// adding it to any environment. Returns a [`PackageHandle`] that
    /// can be inserted into an [`EnvironmentPackages`] set later.
    pub fn register_pypi_package(&mut self, locked_package: PypiPackageData) -> PackageHandle {
        let location = locked_package.location().clone();
        let package_index = if let Some(&existing_idx) = self.pypi_package_indices.get(&location) {
            let LockedPackage::Pypi(pypi_package) = &mut self.packages[existing_idx.0] else {
                panic!("Internal error: Pypi index was pointing to Conda");
            };
            merge_pypi_requires_dist(pypi_package, &locked_package);
            existing_idx
        } else {
            let index = PackageIndex(self.packages.len());
            self.pypi_package_indices.insert(location, index);
            self.packages.push(LockedPackage::Pypi(locked_package));
            index
        };

        PackageHandle::new(package_index, &self.packages[package_index.0])
    }

    /// Registers a conda source package and attaches the provided build and
    /// host environment packages to it.
    ///
    /// Every handle in `build_packages` / `host_packages` must have been
    /// produced by a prior `register_*_package` call on this builder;
    /// passing a handle from another builder returns
    /// [`RegisterSourcePackageError::InvalidHandle`].
    pub fn register_conda_source_package(
        &mut self,
        mut data: CondaSourceData,
        build_packages: impl IntoIterator<Item = PackageHandle>,
        host_packages: impl IntoIterator<Item = PackageHandle>,
    ) -> Result<PackageHandle, RegisterSourcePackageError> {
        data.source_data = self.build_source_data(build_packages, host_packages)?;
        Ok(self.register_conda_package(CondaPackageData::Source(Box::new(data))))
    }

    /// Registers a pypi source package and attaches the provided build and
    /// host environment packages to it.
    ///
    /// Every handle in `build_packages` / `host_packages` must have been
    /// produced by a prior `register_*_package` call on this builder;
    /// passing a handle from another builder returns
    /// [`RegisterSourcePackageError::InvalidHandle`].
    pub fn register_pypi_source_package(
        &mut self,
        mut data: PypiSourceData,
        build_packages: impl IntoIterator<Item = PackageHandle>,
        host_packages: impl IntoIterator<Item = PackageHandle>,
    ) -> Result<PackageHandle, RegisterSourcePackageError> {
        data.source_data = self.build_source_data(build_packages, host_packages)?;
        Ok(self.register_pypi_package(PypiPackageData::Source(Box::new(data))))
    }

    fn build_source_data(
        &self,
        build_packages: impl IntoIterator<Item = PackageHandle>,
        host_packages: impl IntoIterator<Item = PackageHandle>,
    ) -> Result<SourceData, RegisterSourcePackageError> {
        let build_packages: Vec<_> = build_packages.into_iter().collect();
        let host_packages: Vec<_> = host_packages.into_iter().collect();
        for handle in build_packages.iter().chain(host_packages.iter()) {
            handle.get_from_slice(&self.packages)?;
        }
        Ok(SourceData {
            build_packages: EnvironmentPackages::from_handles(build_packages)?,
            host_packages: EnvironmentPackages::from_handles(host_packages)?,
        })
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

    /// Sets the `PyPI` prerelease mode for an environment.
    ///
    /// This function is similar to [`Self::with_pypi_prerelease_mode`] but differs in
    /// that it takes a mutable reference to self instead of consuming it.
    pub fn set_pypi_prerelease_mode(
        &mut self,
        environment: impl Into<String>,
        prerelease_mode: PypiPrereleaseMode,
    ) -> &mut Self {
        self.environment_data(environment)
            .options
            .pypi_prerelease_mode = prerelease_mode;
        self
    }

    /// Sets the `PyPI` prerelease mode for an environment.
    pub fn with_pypi_prerelease_mode(
        mut self,
        environment: impl Into<String>,
        prerelease_mode: PypiPrereleaseMode,
    ) -> Self {
        self.set_pypi_prerelease_mode(environment, prerelease_mode);
        self
    }

    /// Sets the options for an environment.
    pub fn with_options(mut self, environment: impl Into<String>, options: SolveOptions) -> Self {
        self.set_options(environment, options);
        self
    }

    /// Build a [`LockFile`]
    pub fn finish(self) -> LockFile {
        let (environment_lookup, environments) = self
            .environments
            .into_iter()
            .enumerate()
            .map(|(idx, (name, env))| ((name, EnvironmentIndex(idx)), env))
            .unzip();

        LockFile {
            inner: Arc::new(LockFileInner {
                version: FileFormatVersion::LATEST,
                platforms: self.platforms,
                packages: self.packages,
                environments,
                environment_lookup,
            }),
        }
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use rattler_conda_types::{
        MatchSpec, PackageName, PackageRecord, ParseMatchSpecOptions, ParseStrictness::Strict,
        Platform, RepodataRevision, Version, package::DistArchiveIdentifier,
    };
    use url::Url;

    use crate::{
        CondaBinaryData, CondaPackageData, LockFile, PlatformData, PypiPrereleaseMode,
        platform::PlatformName,
    };

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
            .with_platforms(vec![PlatformData {
                name: PlatformName::try_from("linux-64").unwrap(),
                subdir: rattler_conda_types::Platform::Linux64,
                virtual_packages: Vec::new(),
            }])
            .unwrap()
            .with_conda_package(
                "default",
                "linux-64",
                CondaBinaryData {
                    package_record: record.clone(),
                    location: Url::parse(
                        "https://prefix.dev/example/linux-64/foobar-1.0.0-build.tar.bz2",
                    )
                    .unwrap()
                    .into(),
                    file_name: "foobar-1.0.0-build.tar.bz2"
                        .parse::<DistArchiveIdentifier>()
                        .unwrap(),
                    channel: None,
                }
                .into(),
            )
            .unwrap()
            .with_conda_package(
                "default",
                "linux-64",
                CondaBinaryData {
                    package_record: record.clone(),
                    location: Url::parse(
                        "https://prefix.dev/example/linux-64/foobar-1.0.0-build.tar.bz2",
                    )
                    .unwrap()
                    .into(),
                    file_name: "foobar-1.0.0-build.tar.bz2"
                        .parse::<DistArchiveIdentifier>()
                        .unwrap(),
                    channel: None,
                }
                .into(),
            )
            .unwrap()
            .with_conda_package(
                "foobar",
                "linux-64",
                CondaBinaryData {
                    package_record: record_with_purls,
                    location: Url::parse(
                        "https://prefix.dev/example/linux-64/foobar-1.0.0-build.tar.bz2",
                    )
                    .unwrap()
                    .into(),
                    file_name: "foobar-1.0.0-build.tar.bz2"
                        .parse::<DistArchiveIdentifier>()
                        .unwrap(),
                    channel: None,
                }
                .into(),
            )
            .unwrap()
            .finish();
        insta::assert_snapshot!(lock_file.render_to_string().unwrap());
    }

    fn make_run_exports() -> rattler_conda_types::package::RunExportsJson {
        rattler_conda_types::package::RunExportsJson {
            strong: vec!["libfoo >=1.0,<2".into()],
            weak: vec!["libbar >=2".into()],
            ..Default::default()
        }
    }

    fn make_binary_record() -> PackageRecord {
        PackageRecord {
            subdir: "linux-64".into(),
            ..PackageRecord::new(
                PackageName::new_unchecked("foobar"),
                Version::from_str("1.0.0").unwrap(),
                "build".into(),
            )
        }
    }

    fn make_binary(record: PackageRecord) -> CondaBinaryData {
        CondaBinaryData {
            package_record: record,
            location: Url::parse("https://prefix.dev/example/linux-64/foobar-1.0.0-build.tar.bz2")
                .unwrap()
                .into(),
            file_name: "foobar-1.0.0-build.tar.bz2"
                .parse::<DistArchiveIdentifier>()
                .unwrap(),
            channel: None,
        }
    }

    fn binary_run_exports(
        lock_file: &LockFile,
    ) -> Option<rattler_conda_types::package::RunExportsJson> {
        lock_file
            .inner
            .packages
            .iter()
            .find_map(crate::LockedPackage::as_binary_conda)
            .expect("a binary package")
            .package_record
            .run_exports
            .clone()
    }

    fn linux_64_platform() -> PlatformData {
        PlatformData {
            name: PlatformName::try_from("linux-64").unwrap(),
            subdir: rattler_conda_types::Platform::Linux64,
            virtual_packages: Vec::new(),
        }
    }

    #[test]
    fn test_merge_run_exports_unknown_then_known() {
        // Left record has run_exports = None, right record carries Some(non-empty).
        // Merge should adopt the right side's run_exports.
        let unknown = make_binary_record();
        let known = PackageRecord {
            run_exports: Some(make_run_exports()),
            ..make_binary_record()
        };

        let lock_file = LockFile::builder()
            .with_platforms(vec![linux_64_platform()])
            .unwrap()
            .with_conda_package("default", "linux-64", make_binary(unknown).into())
            .unwrap()
            .with_conda_package("default", "linux-64", make_binary(known).into())
            .unwrap()
            .finish();

        assert_eq!(binary_run_exports(&lock_file), Some(make_run_exports()));
    }

    #[test]
    fn test_merge_run_exports_known_empty_blocks_merge() {
        // Left record asserts `Some(empty)` (we know there are no run_exports).
        // A subsequent record carrying Some(non-empty) for the same identifier
        // must NOT override that — first-writer-wins, and `Some(empty)` is a
        // valid claim, distinct from `None`.
        let known_empty = PackageRecord {
            run_exports: Some(rattler_conda_types::package::RunExportsJson::default()),
            ..make_binary_record()
        };
        let known_nonempty = PackageRecord {
            run_exports: Some(make_run_exports()),
            ..make_binary_record()
        };

        let lock_file = LockFile::builder()
            .with_platforms(vec![linux_64_platform()])
            .unwrap()
            .with_conda_package("default", "linux-64", make_binary(known_empty).into())
            .unwrap()
            .with_conda_package("default", "linux-64", make_binary(known_nonempty).into())
            .unwrap()
            .finish();

        assert_eq!(
            binary_run_exports(&lock_file),
            Some(rattler_conda_types::package::RunExportsJson::default())
        );
    }

    #[test]
    fn test_run_exports_roundtrip_binary() {
        // Roundtrip a binary record carrying non-empty run_exports through
        // YAML serialization and parsing.
        let record = PackageRecord {
            run_exports: Some(make_run_exports()),
            ..make_binary_record()
        };

        let lock_file = LockFile::builder()
            .with_platforms(vec![linux_64_platform()])
            .unwrap()
            .with_conda_package("default", "linux-64", make_binary(record).into())
            .unwrap()
            .finish();

        let yaml = lock_file.render_to_string().unwrap();
        let reparsed = LockFile::from_str_with_base_directory(&yaml, None).unwrap();

        assert_eq!(binary_run_exports(&reparsed), Some(make_run_exports()));
    }

    #[test]
    fn test_run_exports_roundtrip_binary_known_empty() {
        // `Some(empty)` must remain `Some(empty)` after a roundtrip — distinct
        // from `None` (= unknown). The lockfile encodes it as `run_exports: {}`.
        let record = PackageRecord {
            run_exports: Some(rattler_conda_types::package::RunExportsJson::default()),
            ..make_binary_record()
        };

        let lock_file = LockFile::builder()
            .with_platforms(vec![linux_64_platform()])
            .unwrap()
            .with_conda_package("default", "linux-64", make_binary(record).into())
            .unwrap()
            .finish();

        let yaml = lock_file.render_to_string().unwrap();
        assert!(
            yaml.contains("run_exports: {}"),
            "expected explicit empty run_exports in YAML:\n{yaml}"
        );

        let reparsed = LockFile::from_str_with_base_directory(&yaml, None).unwrap();
        assert_eq!(
            binary_run_exports(&reparsed),
            Some(rattler_conda_types::package::RunExportsJson::default())
        );
    }

    #[test]
    fn test_run_exports_roundtrip_source() {
        use std::collections::BTreeMap;

        use typed_path::Utf8TypedPathBuf;

        use crate::{CondaPackageData, CondaSourceData, SourceMetadata, UrlOrPath};

        let mut record = PackageRecord::new(
            PackageName::new_unchecked("my-pkg"),
            Version::from_str("0.1.0").unwrap(),
            "py_0".into(),
        );
        record.subdir = "noarch".into();
        record.run_exports = Some(make_run_exports());

        let source = CondaSourceData {
            location: UrlOrPath::Path(Utf8TypedPathBuf::from("./my-pkg")),
            package_build_source: None,
            variants: BTreeMap::new(),
            identifier_hash: None,
            sources: BTreeMap::new(),
            source_data: crate::SourceData::default(),
            metadata: SourceMetadata::Full(Box::new(record)),
        };

        let lock_file = LockFile::builder()
            .with_platforms(vec![linux_64_platform()])
            .unwrap()
            .with_conda_package(
                "default",
                "linux-64",
                CondaPackageData::Source(Box::new(source)),
            )
            .unwrap()
            .finish();

        let yaml = lock_file.render_to_string().unwrap();
        let reparsed = LockFile::from_str_with_base_directory(&yaml, None).unwrap();

        let parsed_source = reparsed
            .inner
            .packages
            .iter()
            .find_map(crate::LockedPackage::as_source_conda)
            .expect("a source package");
        let SourceMetadata::Full(record) = &parsed_source.metadata else {
            panic!("expected Full source metadata");
        };
        assert_eq!(record.run_exports.as_ref(), Some(&make_run_exports()));
    }

    #[test]
    fn test_run_exports_snapshot() {
        // Visual snapshot of how run_exports renders in the lockfile, for both
        // a binary record with a non-empty value and a source record with the
        // same.
        use std::collections::BTreeMap;

        use typed_path::Utf8TypedPathBuf;

        use crate::{CondaPackageData, CondaSourceData, SourceMetadata, UrlOrPath};

        let binary_record = PackageRecord {
            run_exports: Some(make_run_exports()),
            ..make_binary_record()
        };

        let mut source_record = PackageRecord::new(
            PackageName::new_unchecked("my-pkg"),
            Version::from_str("0.1.0").unwrap(),
            "py_0".into(),
        );
        source_record.subdir = "noarch".into();
        source_record.run_exports = Some(rattler_conda_types::package::RunExportsJson {
            strong: vec!["libsource >=3".into()],
            ..Default::default()
        });

        let source = CondaSourceData {
            location: UrlOrPath::Path(Utf8TypedPathBuf::from("./my-pkg")),
            package_build_source: None,
            variants: BTreeMap::new(),
            identifier_hash: None,
            sources: BTreeMap::new(),
            source_data: crate::SourceData::default(),
            metadata: SourceMetadata::Full(Box::new(source_record)),
        };

        let lock_file = LockFile::builder()
            .with_platforms(vec![linux_64_platform()])
            .unwrap()
            .with_conda_package("default", "linux-64", make_binary(binary_record).into())
            .unwrap()
            .with_conda_package(
                "default",
                "linux-64",
                CondaPackageData::Source(Box::new(source)),
            )
            .unwrap()
            .finish();

        insta::assert_snapshot!(lock_file.render_to_string().unwrap());
    }

    #[test]
    fn test_empty_flags_do_not_affect_existing_lock_files() {
        let record = PackageRecord {
            subdir: "linux-64".into(),
            ..PackageRecord::new(
                PackageName::new_unchecked("foobar"),
                Version::from_str("1.0.0").unwrap(),
                "build".into(),
            )
        };
        let package = CondaPackageData::from(CondaBinaryData {
            package_record: record,
            location: Url::parse("https://prefix.dev/example/linux-64/foobar-1.0.0-build.tar.bz2")
                .unwrap()
                .into(),
            file_name: "foobar-1.0.0-build.tar.bz2"
                .parse::<DistArchiveIdentifier>()
                .unwrap(),
            channel: None,
        });

        let lock_file = LockFile::builder()
            .with_platforms(vec![PlatformData {
                name: PlatformName::try_from("linux-64").unwrap(),
                subdir: Platform::Linux64,
                virtual_packages: Vec::new(),
            }])
            .unwrap()
            .with_conda_package("default", "linux-64", package.clone())
            .unwrap()
            .finish();
        let rendered = lock_file.render_to_string().unwrap();
        assert!(!rendered.contains("flags:"));

        let ordinary_spec = MatchSpec::from_str("foobar >=1", Strict).unwrap();
        assert!(package.satisfies(&ordinary_spec));

        let v3_options =
            ParseMatchSpecOptions::strict().with_repodata_revision(RepodataRevision::V3);
        let flag_spec = MatchSpec::from_str("foobar[flags=[cuda]]", v3_options).unwrap();
        assert!(!package.satisfies(&flag_spec));
    }

    #[test]
    fn test_pypi_prerelease_mode() {
        let record = PackageRecord {
            subdir: "linux-64".into(),
            ..PackageRecord::new(
                PackageName::new_unchecked("python"),
                Version::from_str("3.12.0").unwrap(),
                "build".into(),
            )
        };

        let lock_file = LockFile::builder()
            .with_platforms(vec![PlatformData {
                name: PlatformName::try_from("linux-64").unwrap(),
                subdir: rattler_conda_types::Platform::Linux64,
                virtual_packages: Vec::new(),
            }])
            .unwrap()
            .with_conda_package(
                "default",
                "linux-64",
                CondaBinaryData {
                    package_record: record.clone(),
                    location: Url::parse(
                        "https://prefix.dev/example/linux-64/python-3.12.0-build.tar.bz2",
                    )
                    .unwrap()
                    .into(),
                    file_name: "python-3.12.0-build.tar.bz2"
                        .parse::<DistArchiveIdentifier>()
                        .unwrap(),
                    channel: None,
                }
                .into(),
            )
            .unwrap()
            .with_pypi_prerelease_mode("default", PypiPrereleaseMode::Allow)
            .finish();

        // Verify the prerelease mode is set correctly
        let env = lock_file.environment("default").unwrap();
        assert_eq!(env.pypi_prerelease_mode(), PypiPrereleaseMode::Allow);

        // Verify it serializes correctly
        insta::assert_snapshot!(lock_file.render_to_string().unwrap());
    }

    #[test]
    fn test_pypi_prerelease_mode_roundtrip() {
        let record = PackageRecord {
            subdir: "linux-64".into(),
            ..PackageRecord::new(
                PackageName::new_unchecked("python"),
                Version::from_str("3.12.0").unwrap(),
                "build".into(),
            )
        };

        // Test various prerelease modes
        for mode in [
            PypiPrereleaseMode::Disallow,
            PypiPrereleaseMode::Allow,
            PypiPrereleaseMode::IfNecessary,
            PypiPrereleaseMode::Explicit,
            PypiPrereleaseMode::IfNecessaryOrExplicit,
        ] {
            let lock_file = LockFile::builder()
                .with_platforms(vec![PlatformData {
                    name: PlatformName::try_from("linux-64").unwrap(),
                    subdir: rattler_conda_types::Platform::Linux64,
                    virtual_packages: Vec::new(),
                }])
                .unwrap()
                .with_conda_package(
                    "default",
                    "linux-64",
                    CondaBinaryData {
                        package_record: record.clone(),
                        location: Url::parse(
                            "https://prefix.dev/example/linux-64/python-3.12.0-build.tar.bz2",
                        )
                        .unwrap()
                        .into(),
                        file_name: "python-3.12.0-build.tar.bz2"
                            .parse::<DistArchiveIdentifier>()
                            .unwrap(),
                        channel: None,
                    }
                    .into(),
                )
                .unwrap()
                .with_pypi_prerelease_mode("default", mode)
                .finish();

            // Serialize
            let rendered = lock_file.render_to_string().unwrap();

            // Parse again
            let parsed = LockFile::from_str_with_base_directory(&rendered, None).unwrap();

            // Verify the prerelease mode round trips correctly
            assert_eq!(
                parsed
                    .environment("default")
                    .unwrap()
                    .pypi_prerelease_mode(),
                mode
            );
        }
    }

    #[test]
    fn register_conda_source_package_attaches_build_and_host() {
        use std::collections::BTreeMap;

        use crate::{CondaSourceData, SourceMetadata};

        let make_binary = |name: &str| {
            let mut record = PackageRecord::new(
                PackageName::new_unchecked(name),
                Version::from_str("1.0.0").unwrap(),
                "build0".into(),
            );
            record.subdir = "linux-64".into();
            CondaBinaryData {
                package_record: record,
                location: Url::parse(&format!(
                    "https://example.com/linux-64/{name}-1.0.0-build0.tar.bz2"
                ))
                .unwrap()
                .into(),
                file_name: format!("{name}-1.0.0-build0.tar.bz2")
                    .parse::<DistArchiveIdentifier>()
                    .unwrap(),
                channel: None,
            }
        };

        let mut builder = LockFile::builder();
        let compiler = builder.register_conda_package(make_binary("compiler").into());
        let runtime = builder.register_conda_package(make_binary("runtime").into());

        let source = CondaSourceData {
            location: crate::UrlOrPath::Path("./my-pkg".into()),
            package_build_source: None,
            variants: BTreeMap::new(),
            identifier_hash: None,
            sources: BTreeMap::new(),
            source_data: crate::SourceData::default(),
            metadata: SourceMetadata::Full(Box::new(PackageRecord::new(
                PackageName::new_unchecked("my-pkg"),
                Version::from_str("0.1.0").unwrap(),
                "py_0".into(),
            ))),
        };

        let handle = builder
            .register_conda_source_package(source, [compiler.clone()], [runtime.clone()])
            .unwrap();

        let lock_file = builder.finish();
        let packages = &lock_file.inner.packages;
        let source_data = packages[handle.index.0]
            .as_source_conda()
            .expect("registered package is a source conda package")
            .source_data
            .clone();
        assert_eq!(
            source_data.build_packages.to_selector_ids(),
            vec![compiler.selector_id]
        );
        assert_eq!(
            source_data.host_packages.to_selector_ids(),
            vec![runtime.selector_id]
        );
    }

    #[test]
    fn register_conda_source_package_rejects_foreign_handle() {
        use std::collections::BTreeMap;

        use crate::{CondaSourceData, SourceMetadata};

        let binary = CondaBinaryData {
            package_record: {
                let mut r = PackageRecord::new(
                    PackageName::new_unchecked("other"),
                    Version::from_str("1.0.0").unwrap(),
                    "build0".into(),
                );
                r.subdir = "linux-64".into();
                r
            },
            location: Url::parse("https://example.com/linux-64/other-1.0.0-build0.tar.bz2")
                .unwrap()
                .into(),
            file_name: "other-1.0.0-build0.tar.bz2"
                .parse::<DistArchiveIdentifier>()
                .unwrap(),
            channel: None,
        };

        let mut foreign = LockFile::builder();
        let foreign_handle = foreign.register_conda_package(binary.into());

        let mut builder = LockFile::builder();
        let source = CondaSourceData {
            location: crate::UrlOrPath::Path("./my-pkg".into()),
            package_build_source: None,
            variants: BTreeMap::new(),
            identifier_hash: None,
            sources: BTreeMap::new(),
            source_data: crate::SourceData::default(),
            metadata: SourceMetadata::Full(Box::new(PackageRecord::new(
                PackageName::new_unchecked("my-pkg"),
                Version::from_str("0.1.0").unwrap(),
                "py_0".into(),
            ))),
        };

        let result = builder.register_conda_source_package(source, [foreign_handle], []);
        assert!(matches!(
            result,
            Err(crate::RegisterSourcePackageError::InvalidHandle(_))
        ));
    }

    #[test]
    fn lock_file_with_conda_and_pypi_source_packages_serializes_all_packages() {
        use std::collections::BTreeMap;

        use pep508_rs::PackageName as PypiPackageName;
        use typed_path::Utf8TypedPathBuf;

        use crate::{
            CondaPackageData, CondaSourceData, LockedPackage, PypiPackageData, PypiSourceData,
            SourceMetadata, UrlOrPath, Verbatim,
        };

        let make_binary = |name: &str| {
            let mut record = PackageRecord::new(
                PackageName::new_unchecked(name),
                Version::from_str("1.0.0").unwrap(),
                "build0".into(),
            );
            record.subdir = "linux-64".into();
            // Use a path location: path-based locations derive no channel,
            // which keeps the serialized YAML free of the `channel: null`
            // quirk triggered by URL-based binaries with explicit
            // `channel: None`.
            CondaBinaryData {
                package_record: record,
                location: UrlOrPath::Path(Utf8TypedPathBuf::from(format!(
                    "./{name}-1.0.0-build0.tar.bz2"
                ))),
                file_name: format!("{name}-1.0.0-build0.tar.bz2")
                    .parse::<DistArchiveIdentifier>()
                    .unwrap(),
                channel: None,
            }
        };

        let make_conda_source = |name: &str| CondaSourceData {
            location: UrlOrPath::Path(Utf8TypedPathBuf::from(format!("./{name}"))),
            package_build_source: None,
            variants: BTreeMap::new(),
            identifier_hash: None,
            sources: BTreeMap::new(),
            source_data: crate::SourceData::default(),
            metadata: SourceMetadata::Full(Box::new({
                let mut record = PackageRecord::new(
                    PackageName::new_unchecked(name),
                    Version::from_str("0.1.0").unwrap(),
                    "py_0".into(),
                );
                record.subdir = "noarch".into();
                record
            })),
        };

        let make_pypi_source = |name: &str| PypiSourceData {
            name: PypiPackageName::from_str(name).unwrap(),
            location: Verbatim::new(UrlOrPath::Path(Utf8TypedPathBuf::from(format!("./{name}")))),
            requires_dist: vec![],
            requires_python: None,
            source_data: crate::SourceData::default(),
        };

        let mut builder = LockFile::builder()
            .with_platforms(vec![PlatformData {
                name: PlatformName::try_from("linux-64").unwrap(),
                subdir: rattler_conda_types::Platform::Linux64,
                virtual_packages: Vec::new(),
            }])
            .unwrap();

        // Register the four binary packages that will serve as build/host
        // environments for the two source packages.
        let conda_compiler = builder.register_conda_package(make_binary("conda-compiler").into());
        let conda_runtime = builder.register_conda_package(make_binary("conda-runtime").into());
        let pypi_builder = builder.register_conda_package(make_binary("pypi-build-tool").into());
        let pypi_runtime = builder.register_conda_package(make_binary("pypi-runtime").into());

        // Pre-build the source packages with their build/host environments
        // attached. Sharing the same `CondaSourceData` / `PypiSourceData`
        // instance between `register_*_source_package` and the later
        // `add_*_package` call ensures both registrations compute the same
        // `SourceIdentifier` (which now incorporates build/host packages)
        // and dedup to a single entry.
        let conda_source_with_env = {
            let mut data = make_conda_source("my-conda-pkg");
            data.source_data = crate::SourceData {
                build_packages: crate::EnvironmentPackages::from_handles([conda_compiler.clone()])
                    .unwrap(),
                host_packages: crate::EnvironmentPackages::from_handles([conda_runtime.clone()])
                    .unwrap(),
            };
            data
        };
        let pypi_source_with_env = {
            let mut data = make_pypi_source("my-pypi-pkg");
            data.source_data = crate::SourceData {
                build_packages: crate::EnvironmentPackages::from_handles([pypi_builder.clone()])
                    .unwrap(),
                host_packages: crate::EnvironmentPackages::from_handles([pypi_runtime.clone()])
                    .unwrap(),
            };
            data
        };

        // Register the two source packages. Use `register_conda_package`
        // / `register_pypi_package` directly: the convenience wrappers
        // (`register_*_source_package`) overwrite `source_data` with the
        // build/host packages they receive as arguments, which would erase
        // the pre-populated `source_data` we just built.
        let conda_source_handle = builder.register_conda_package(CondaPackageData::Source(
            Box::new(conda_source_with_env.clone()),
        ));
        let pypi_source_handle = builder.register_pypi_package(PypiPackageData::Source(Box::new(
            pypi_source_with_env.clone(),
        )));

        // An extra package that nothing references — neither added to an
        // environment, nor used as a build/host/runtime dependency. It must
        // be stripped from the serialized output.
        let orphan = builder.register_conda_package(make_binary("orphan").into());

        // Make every registered package reachable from the environment so it
        // actually appears in the serialized packages list. The source
        // packages reuse the exact same `*_source_with_env` value used above
        // so they dedup against the already-registered entry.
        builder
            .add_conda_package("default", "linux-64", make_binary("conda-compiler").into())
            .unwrap()
            .add_conda_package("default", "linux-64", make_binary("conda-runtime").into())
            .unwrap()
            .add_conda_package("default", "linux-64", make_binary("pypi-build-tool").into())
            .unwrap()
            .add_conda_package("default", "linux-64", make_binary("pypi-runtime").into())
            .unwrap()
            .add_conda_package(
                "default",
                "linux-64",
                CondaPackageData::Source(Box::new(conda_source_with_env)),
            )
            .unwrap()
            .add_pypi_package(
                "default",
                "linux-64",
                PypiPackageData::Source(Box::new(pypi_source_with_env)),
            )
            .unwrap();

        let lock_file = builder.finish();

        // The two source packages still carry the source_data populated by
        // `register_*_source_package` — deduplication kept the first
        // registration and its build/host environments.
        let conda_source = lock_file.inner.packages[conda_source_handle.index.0]
            .as_source_conda()
            .unwrap();
        assert_eq!(
            conda_source.source_data.build_packages.to_selector_ids(),
            vec![conda_compiler.selector_id.clone()]
        );
        assert_eq!(
            conda_source.source_data.host_packages.to_selector_ids(),
            vec![conda_runtime.selector_id.clone()]
        );
        let pypi_source = lock_file.inner.packages[pypi_source_handle.index.0]
            .as_pypi()
            .and_then(crate::PypiPackageData::as_source)
            .unwrap();
        assert_eq!(
            pypi_source.source_data.build_packages.to_selector_ids(),
            vec![pypi_builder.selector_id.clone()]
        );
        assert_eq!(
            pypi_source.source_data.host_packages.to_selector_ids(),
            vec![pypi_runtime.selector_id.clone()]
        );

        // Serialize and check all six reachable packages appear in the YAML.
        let yaml = lock_file.render_to_string().unwrap();
        for expected in [
            &conda_compiler.selector_id,
            &conda_runtime.selector_id,
            &pypi_builder.selector_id,
            &pypi_runtime.selector_id,
            &conda_source_handle.selector_id,
            &pypi_source_handle.selector_id,
        ] {
            assert!(
                yaml.contains(expected.as_str()),
                "expected selector id {} in rendered YAML:\n{yaml}",
                expected.as_str()
            );
        }

        // The orphan package must be stripped from the serialized output.
        assert!(
            !yaml.contains(orphan.selector_id.as_str()),
            "unreferenced package {} should not appear in the rendered YAML:\n{yaml}",
            orphan.selector_id.as_str()
        );

        // Top-level packages list must contain all six (two source + four
        // binary), each appearing exactly once as a `packages` entry.
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let top_level = parsed["packages"].as_sequence().unwrap();
        assert_eq!(
            top_level.len(),
            6,
            "expected 6 top-level packages, got {}:\n{yaml}",
            top_level.len()
        );

        // Deserializing must succeed and expose the same six packages plus
        // the build/host references on both source packages.
        let reparsed = LockFile::from_str_with_base_directory(&yaml, None).unwrap();
        assert_eq!(reparsed.inner.packages.len(), 6);
        let reparsed_conda_source = reparsed
            .inner
            .packages
            .iter()
            .find_map(LockedPackage::as_source_conda)
            .expect("conda source package survives round-trip");
        assert_eq!(
            reparsed_conda_source.source_data.build_packages.len(),
            1,
            "conda source kept its build package"
        );
        assert_eq!(
            reparsed_conda_source.source_data.host_packages.len(),
            1,
            "conda source kept its host package"
        );
        let reparsed_pypi_source = reparsed
            .inner
            .packages
            .iter()
            .find_map(|p| p.as_pypi().and_then(crate::PypiPackageData::as_source))
            .expect("pypi source package survives round-trip");
        assert_eq!(
            reparsed_pypi_source.source_data.build_packages.len(),
            1,
            "pypi source kept its build package"
        );
        assert_eq!(
            reparsed_pypi_source.source_data.host_packages.len(),
            1,
            "pypi source kept its host package"
        );

        // Two full round-trips must reproduce the original YAML byte-for-byte.
        let yaml_after_first = LockFile::from_str_with_base_directory(&yaml, None)
            .unwrap()
            .render_to_string()
            .unwrap();
        similar_asserts::assert_eq!(yaml, yaml_after_first);
        let yaml_after_second = LockFile::from_str_with_base_directory(&yaml_after_first, None)
            .unwrap()
            .render_to_string()
            .unwrap();
        similar_asserts::assert_eq!(yaml, yaml_after_second);
    }
}
