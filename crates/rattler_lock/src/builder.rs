//! Builder for the creation of lock files.

use std::{borrow::Cow, collections::HashMap, sync::Arc};

use indexmap::{IndexMap, IndexSet};
use rattler_conda_types::Version;

use crate::{
    file_format_version::FileFormatVersion, Channel, CondaBinaryData, CondaPackageData,
    CondaSourceData, EnvironmentData, EnvironmentPackageData, LockFile, LockFileInner,
    LockedPackageRef, ParseCondaLockError, PypiIndexes, PypiPackageData, SolveOptions, UrlOrPath,
};

/// Information about a single locked package in an environment.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum LockedPackage {
    /// A conda package
    Conda(CondaPackageData),

    /// A pypi package in an environment
    Pypi(PypiPackageData),
}

impl From<LockedPackageRef<'_>> for LockedPackage {
    fn from(value: LockedPackageRef<'_>) -> Self {
        match value {
            LockedPackageRef::Conda(data) => LockedPackage::Conda(data.clone()),
            LockedPackageRef::Pypi(data) => LockedPackage::Pypi(data.clone()),
        }
    }
}

impl From<CondaPackageData> for LockedPackage {
    fn from(value: CondaPackageData) -> Self {
        LockedPackage::Conda(value)
    }
}

impl From<PypiPackageData> for LockedPackage {
    fn from(data: PypiPackageData) -> Self {
        LockedPackage::Pypi(data)
    }
}

impl LockedPackage {
    /// Returns the name of the package as it occurs in the lock file. This
    /// might not be the normalized name.
    pub fn name(&self) -> &str {
        match self {
            LockedPackage::Conda(data) => data.name().as_source(),
            LockedPackage::Pypi(data) => data.name.as_ref(),
        }
    }

    /// Returns the location of the package.
    pub fn location(&self) -> &UrlOrPath {
        match self {
            LockedPackage::Conda(data) => data.location(),
            LockedPackage::Pypi(data) => &data.location,
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
    platforms: Vec<crate::PlatformData>,

    /// Metadata about the different environments stored in the lock file.
    environments: IndexMap<String, EnvironmentData>,

    /// All conda packages stored in the lock file.
    conda_packages: Vec<CondaPackageData>,

    /// Maps unique binary package identifiers to their index in `conda_packages`.
    /// Used for deduplication of binary packages.
    binary_package_indices: HashMap<UniqueBinaryIdentifier, usize>,

    pypi_packages: IndexSet<PypiPackageData>,
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

impl LockFileBuilder {
    /// Generate a new lock file using the builder pattern
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the `Vec<Platform>` into the `LockFile`, replacing any platforms that were
    /// known before.
    pub fn with_platforms(
        mut self,
        platforms: Vec<crate::PlatformData>,
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
    pub fn add_platform(
        mut self,
        platform: crate::PlatformData,
    ) -> Result<Self, ParseCondaLockError> {
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

    fn find_platform_index(&self, platform_name: &str) -> Result<usize, ()> {
        if let Some(platform_index) = self
            .platforms
            .iter()
            .position(|p| p.name.as_str() == platform_name)
        {
            Ok(platform_index)
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
        let package_idx = match &locked_package {
            CondaPackageData::Binary(binary_data) => {
                let unique_identifier = UniqueBinaryIdentifier::from(binary_data);

                // Check if we already have this binary package
                if let Some(&existing_idx) = self.binary_package_indices.get(&unique_identifier) {
                    // Merge with existing package
                    if let CondaPackageData::Binary(existing) =
                        &mut self.conda_packages[existing_idx]
                    {
                        if let Cow::Owned(merged) = existing.merge(binary_data) {
                            *existing = merged;
                        }
                    }
                    existing_idx
                } else {
                    // Add new binary package
                    let idx = self.conda_packages.len();
                    self.conda_packages.push(locked_package);
                    self.binary_package_indices.insert(unique_identifier, idx);
                    idx
                }
            }
            CondaPackageData::Source(_) => {
                // Source packages are never merged, just appended
                let idx = self.conda_packages.len();
                self.conda_packages.push(locked_package);
                idx
            }
        };

        // Add the package to the environment that it is intended for.
        self.environment_data(environment)
            .packages
            .entry(platform_index)
            .or_default()
            .insert(EnvironmentPackageData::Conda(package_idx));

        Ok(self)
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

        // Add the package to the list of packages.
        let package_idx = self.pypi_packages.insert_full(locked_package).0;

        // Add the package to the environment that it is intended for.
        self.environment_data(environment)
            .packages
            .entry(platform_index)
            .or_default()
            .insert(EnvironmentPackageData::Pypi(package_idx));

        Ok(self)
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
        prerelease_mode: crate::PypiPrereleaseMode,
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
        prerelease_mode: crate::PypiPrereleaseMode,
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
            .map(|(idx, (name, env))| ((name, idx), env))
            .unzip();

        LockFile {
            inner: Arc::new(LockFileInner {
                version: FileFormatVersion::LATEST,
                platforms: self.platforms,
                conda_packages: self.conda_packages,
                pypi_packages: self.pypi_packages.into_iter().collect(),
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
        package::DistArchiveIdentifier, PackageName, PackageRecord, Version,
    };
    use url::Url;

    use crate::{platform::PlatformName, CondaBinaryData, LockFile, PypiPrereleaseMode};

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
            .with_platforms(vec![crate::PlatformData {
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
            .with_platforms(vec![crate::PlatformData {
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
                .with_platforms(vec![crate::PlatformData {
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
}
