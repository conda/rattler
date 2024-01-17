//! Builder for the creation of lock files. Currently,
//!
use crate::{
    package::{CondaPackageData, PypiPackageData, RuntimePackageData},
    Channel, EnvironmentData, LockFile,
};
use fxhash::FxHashMap;
use indexmap::IndexSet;
use rattler_conda_types::Platform;
use std::collections::HashMap;

/// A struct to incrementally build a lock-file.
#[derive(Default)]
pub struct LockFileBuilder {
    /// Metadata about the different environments stored in the lock file.
    environments: FxHashMap<String, EnvironmentData>,

    /// A list of all package metadata stored in the lock file.
    conda_packages: IndexSet<CondaPackageData>,
    pypi_packages: IndexSet<PypiPackageData>,
}

impl LockFileBuilder {
    /// Generate a new lock file using the builder pattern
    pub fn new() -> Self {
        Self::default()
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
            })
            .channels = channels.into_iter().map(Into::into).collect();
        self
    }

    /// Adds a specific locked package to a specific environment and platform.
    ///
    /// This function is similar to [`with_conda_package`] but differs in that it takes a mutable
    /// reference to self instead of consuming it. This allows for a more fluent with chaining
    /// calls.
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
            });

        // Add the package to the list of packages.
        let package_idx =
            RuntimePackageData::Conda(self.conda_packages.insert_full(locked_package).0);

        // Add the package to the environment that it is intended for.
        environment
            .packages
            .entry(platform)
            .or_default()
            .push(package_idx);

        self
    }

    /// Adds a specific locked package to a specific environment and platform.
    ///
    /// This function is similar to [`add_conda_package`] but differs in that it consumes `self` instead
    /// of taking a mutable reference. This allows for a better interface when modifying an existing
    /// instance.
    pub fn with_conda_package(
        mut self,
        environment: impl Into<String>,
        platform: Platform,
        locked_package: CondaPackageData,
    ) -> Self {
        self.add_conda_package(environment, platform, locked_package);
        self
    }

    /// Adds an environment to the builder.
    pub fn with_channels(
        &mut self,
        environment: impl Into<String>,
        channels: impl IntoIterator<Item = impl Into<Channel>>,
    ) -> &mut Self {
        self.set_channels(environment, channels);
        self
    }

    /// Build a [`LockFile`]
    pub fn finish(self) -> LockFile {
        LockFile {
            conda_packages: self.conda_packages.into_iter().collect(),
            pypi_packages: self.pypi_packages.into_iter().collect(),
            environments: self.environments,
        }
    }
}
//
// #[cfg(test)]
// mod tests {
//     use chrono::Utc;
//     use std::str::FromStr;
//
//     use crate::builder::{CondaLockedDependencyBuilder, LockFileBuilder, LockedPackagesBuilder};
//     use crate::PackageHashes;
//     use rattler_conda_types::{
//         ChannelConfig, MatchSpec, NoArchType, PackageName, Platform, RepoDataRecord,
//     };
//     use rattler_digest::parse_digest_from_hex;
//
//     #[test]
//     fn conda_lock_builder_and_conversions() {
//         let _channel_config = ChannelConfig::default();
//         let lock = LockFileBuilder::new(
//             ["conda_forge"],
//             [Platform::Osx64],
//             [MatchSpec::from_str("python =3.11.0").unwrap()]
//         )
//             .add_locked_packages(LockedPackagesBuilder::new(Platform::Osx64)
//                 .with_locked_package(CondaLockedDependencyBuilder {
//                     name: PackageName::new_unchecked("python"),
//                     version: "3.11.0".to_string(),
//                     build: "h4150a38_1_cpython".to_string(),
//                     url: "https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda".parse().unwrap(),
//                     package_hashes:  PackageHashes::Md5Sha256(parse_digest_from_hex::<rattler_digest::Md5>("c6f4b87020c72e2700e3e94c1fc93b70").unwrap(),
//                                                                parse_digest_from_hex::<rattler_digest::Sha256>("7c58de8c7d98b341bd9be117feec64782e704fec5c30f6e14713ebccaab9b5d8").unwrap()),
//                     dependency_list: vec![String::from("python 3.11.0.*")],
//                     optional: None,
//                     arch: Some("x86_64".to_string()),
//                     subdir: Some("noarch".to_string()),
//                     build_number: 12,
//                     constrains: vec!["bla".to_string()],
//                     features: Some("foobar".to_string()),
//                     track_features: vec!["dont-track".to_string()],
//                     license: Some("BSD-3-Clause".to_string()),
//                     license_family: Some("BSD".to_string()),
//                     noarch: NoArchType::python(),
//                     size: Some(12000),
//                     timestamp: Some(Utc::now()),
//                     purls: vec![
//                         "pkg:deb/debian/python@3.11.0?arch=x86_64".parse().unwrap(),
//                     ]
//                 }))
//             .build().unwrap();
//
//         // Convert to RepoDataRecord
//         let locked_dep = lock.package.first().unwrap();
//         let record = RepoDataRecord::try_from(locked_dep).unwrap();
//
//         assert_eq!(
//             record.package_record.name.as_source(),
//             locked_dep.name.as_str()
//         );
//         assert_eq!(
//             record.channel,
//             "https://conda.anaconda.org/conda-forge".to_string()
//         );
//         assert_eq!(
//             record.file_name,
//             "python-3.11.0-h4150a38_1_cpython.conda".to_string()
//         );
//         assert_eq!(
//             record.package_record.version.to_string(),
//             locked_dep.version
//         );
//         assert_eq!(
//             Some(&record.package_record.build),
//             locked_dep.as_conda().unwrap().build.as_ref()
//         );
//         assert_eq!(
//             record.package_record.platform.clone().unwrap(),
//             locked_dep.platform.only_platform().unwrap()
//         );
//         assert_eq!(
//             record.package_record.arch,
//             locked_dep.as_conda().unwrap().arch
//         );
//         assert_eq!(
//             Some(&record.package_record.subdir),
//             locked_dep.as_conda().unwrap().subdir.as_ref()
//         );
//         assert_eq!(
//             Some(record.package_record.build_number),
//             locked_dep.as_conda().unwrap().build_number
//         );
//         assert_eq!(
//             record.package_record.constrains,
//             locked_dep.as_conda().unwrap().constrains.clone()
//         );
//         assert_eq!(
//             record.package_record.features,
//             locked_dep.as_conda().unwrap().features
//         );
//         assert_eq!(
//             record.package_record.track_features,
//             locked_dep.as_conda().unwrap().track_features
//         );
//         assert_eq!(
//             record.package_record.license_family,
//             locked_dep.as_conda().unwrap().license_family
//         );
//         assert_eq!(
//             record.package_record.noarch,
//             locked_dep.as_conda().unwrap().noarch
//         );
//         assert_eq!(
//             record.package_record.size,
//             locked_dep.as_conda().unwrap().size
//         );
//         assert_eq!(
//             record.package_record.timestamp,
//             locked_dep.as_conda().unwrap().timestamp
//         );
//
//         // Convert to LockedDependency
//         let locked_package = CondaLockedDependencyBuilder::try_from(record.clone()).unwrap();
//         assert_eq!(record.package_record.name, locked_package.name);
//         assert_eq!(
//             record.package_record.version.to_string(),
//             locked_package.version
//         );
//         assert_eq!(&record.package_record.build, &locked_package.build);
//         assert_eq!(record.package_record.arch, locked_package.arch);
//         assert_eq!(
//             record.package_record.subdir,
//             locked_package.subdir.clone().unwrap_or_default()
//         );
//         assert_eq!(
//             record.package_record.build_number,
//             locked_package.build_number
//         );
//         assert_eq!(record.package_record.constrains, locked_package.constrains);
//         assert_eq!(record.package_record.features, locked_package.features);
//         assert_eq!(
//             record.package_record.license_family,
//             locked_package.license_family
//         );
//         assert_eq!(record.package_record.noarch, locked_package.noarch);
//         assert_eq!(record.package_record.size, locked_package.size);
//         assert_eq!(record.package_record.timestamp, locked_package.timestamp);
//     }
// }
