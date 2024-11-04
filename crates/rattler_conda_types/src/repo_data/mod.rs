//! Defines [`RepoData`]. `RepoData` stores information of all packages present
//! in a subdirectory of a channel. It provides indexing functionality.

pub mod patches;
pub mod sharded;
mod topological_sort;

use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter},
    path::Path,
};

use fxhash::{FxHashMap, FxHashSet};
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use rattler_macros::sorted;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, OneOrMany};
use thiserror::Error;
use url::Url;

use crate::{
    build_spec::BuildNumber,
    package::{IndexJson, RunExportsJson},
    utils::{
        serde::{sort_map_alphabetically, DeserializeFromStrUnchecked},
        url::add_trailing_slash,
    },
    Channel, MatchSpec, Matches, NoArchType, PackageName, PackageUrl, ParseMatchSpecError,
    ParseStrictness, Platform, RepoDataRecord, VersionWithSource,
};

/// [`RepoData`] is an index of package binaries available on in a subdirectory
/// of a Conda channel.
// Note: we cannot use the sorted macro here, because the `packages` and `conda_packages` fields are
// serialized in a special way. Therefore we do it manually.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct RepoData {
    /// The channel information contained in the repodata.json file
    pub info: Option<ChannelInfo>,

    /// The tar.bz2 packages contained in the repodata.json file
    #[serde(default, serialize_with = "sort_map_alphabetically")]
    pub packages: FxHashMap<String, PackageRecord>,

    /// The conda packages contained in the repodata.json file (under a
    /// different key for backwards compatibility with previous conda
    /// versions)
    #[serde(
        default,
        rename = "packages.conda",
        serialize_with = "sort_map_alphabetically"
    )]
    pub conda_packages: FxHashMap<String, PackageRecord>,

    /// removed packages (files are still accessible, but they are not
    /// installable like regular packages)
    #[serde(
        default,
        serialize_with = "sort_set_alphabetically",
        skip_serializing_if = "FxHashSet::is_empty"
    )]
    pub removed: FxHashSet<String>,

    /// The version of the repodata format
    #[serde(rename = "repodata_version")]
    pub version: Option<u64>,
}

/// Information about subdirectory of channel in the Conda [`RepoData`]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct ChannelInfo {
    /// The channel's subdirectory
    pub subdir: String,

    /// The `base_url` for all package urls. Can be an absolute or relative url.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// A single record in the Conda repodata. A single record refers to a single
/// binary distribution of a package on a Conda channel.
#[serde_as]
#[skip_serializing_none]
#[sorted]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Hash)]
pub struct PackageRecord {
    /// Optionally the architecture the package supports
    pub arch: Option<String>,

    /// The build string of the package
    pub build: String,

    /// The build number of the package
    pub build_number: BuildNumber,

    /// Additional constraints on packages. `constrains` are different from
    /// `depends` in that packages specified in `depends` must be installed
    /// next to this package, whereas packages specified in `constrains` are
    /// not required to be installed, but if they are installed they must follow
    /// these constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constrains: Vec<String>,

    /// Specification of packages this package depends on
    #[serde(default)]
    pub depends: Vec<String>,

    /// Features are a deprecated way to specify different feature sets for the
    /// conda solver. This is not supported anymore and should not be used.
    /// Instead, `mutex` packages should be used to specify
    /// mutually exclusive features.
    pub features: Option<String>,

    /// A deprecated md5 hash
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Md5>>")]
    pub legacy_bz2_md5: Option<Md5Hash>,

    /// A deprecated package archive size.
    pub legacy_bz2_size: Option<u64>,

    /// The specific license of the package
    pub license: Option<String>,

    /// The license family
    pub license_family: Option<String>,

    /// Optionally a MD5 hash of the package archive
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,

    /// The name of the package
    #[serde_as(deserialize_as = "DeserializeFromStrUnchecked")]
    pub name: PackageName,

    /// If this package is independent of architecture this field specifies in
    /// what way. See [`NoArchType`] for more information.
    #[serde(skip_serializing_if = "NoArchType::is_none")]
    pub noarch: NoArchType,

    /// Optionally the platform the package supports
    pub platform: Option<String>, // Note that this does not match the [`Platform`] enum..

    /// Package identifiers of packages that are equivalent to this package but
    /// from other ecosystems.
    /// starting from 0.23.2, this field became [`Option<Vec<PackageUrl>>`].
    /// This was done to support older lockfiles,
    /// where we didn't differentiate between empty purl and missing one.
    /// Now, None:: means that the purl is missing, and it will be tried to
    /// filled in. So later it can be one of the following:
    /// [`Some(vec![])`] means that the purl is empty and package is not pypi
    /// one. [`Some([`PackageUrl`])`] means that it is a pypi package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purls: Option<BTreeSet<PackageUrl>>,

    /// Optionally a path within the environment of the site-packages directory.
    /// This field is only present for python interpreter packages.
    /// This field was introduced with <https://github.com/conda/ceps/blob/main/cep-17.md>.
    pub python_site_packages_path: Option<String>,

    /// Run exports that are specified in the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_exports: Option<RunExportsJson>,

    /// Optionally a SHA256 hash of the package archive
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,

    /// Optionally the size of the package archive in bytes
    pub size: Option<u64>,

    /// The subdirectory where the package can be found
    #[serde(default)]
    pub subdir: String,

    /// The date this entry was created.
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Track features are nowadays only used to downweight packages (ie. give
    /// them less priority). To that effect, the number of track features is
    /// counted (number of commas) and the package is downweighted
    /// by the number of track_features.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "OneOrMany<_>")]
    pub track_features: Vec<String>,

    /// The version of the package
    pub version: VersionWithSource,
    // Looking at the `PackageRecord` class in the Conda source code a record can also include all
    // these fields. However, I have no idea if or how they are used so I left them out.
    //pub preferred_env: Option<String>,
    //pub date: Option<String>,
    //pub package_type: ?
}

impl Display for PackageRecord {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.build.is_empty() {
            write!(f, "{} {}", self.name.as_normalized(), self.version,)
        } else {
            write!(
                f,
                "{}={}={}",
                self.name.as_normalized(),
                self.version,
                self.build
            )
        }
    }
}

impl RepoData {
    /// Parses [`RepoData`] from a file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let contents = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&contents)?)
    }

    /// Returns the `base_url` specified in the repodata.
    pub fn base_url(&self) -> Option<&str> {
        self.info.as_ref().and_then(|i| i.base_url.as_deref())
    }

    /// Builds a [`Vec<RepoDataRecord>`] from the packages in a [`RepoData`]
    /// given the source of the data.
    pub fn into_repo_data_records(self, channel: &Channel) -> Vec<RepoDataRecord> {
        let mut records = Vec::with_capacity(self.packages.len() + self.conda_packages.len());
        let channel_name = channel.canonical_name();
        let base_url = self.base_url().map(ToOwned::to_owned);

        // Determine the base_url of the channel
        for (filename, package_record) in self.packages.into_iter().chain(self.conda_packages) {
            records.push(RepoDataRecord {
                url: compute_package_url(
                    &channel
                        .base_url()
                        .join(&package_record.subdir)
                        .expect("cannot join channel base_url and subdir"),
                    base_url.as_deref(),
                    &filename,
                ),
                channel: channel_name.clone(),
                package_record,
                file_name: filename,
            });
        }
        records
    }
}

/// Computes the URL for a package.
pub fn compute_package_url(
    repo_data_base_url: &Url,
    base_url: Option<&str>,
    filename: &str,
) -> Url {
    let mut absolute_url = match base_url {
        None => repo_data_base_url.clone(),
        Some(base_url) => match Url::parse(base_url) {
            Err(url::ParseError::RelativeUrlWithoutBase) if !base_url.starts_with('/') => {
                add_trailing_slash(repo_data_base_url)
                    .join(base_url)
                    .expect("failed to join base_url with channel")
            }
            Err(url::ParseError::RelativeUrlWithoutBase) => {
                let mut url = repo_data_base_url.clone();
                url.set_path(base_url);
                url
            }
            Err(e) => unreachable!("{e}"),
            Ok(base_url) => base_url,
        },
    };

    let path = absolute_url.path();
    if !path.ends_with('/') {
        absolute_url.set_path(&format!("{path}/"));
    }
    absolute_url
        .join(filename)
        .expect("failed to join base_url and filename")
}

impl AsRef<PackageRecord> for PackageRecord {
    fn as_ref(&self) -> &PackageRecord {
        self
    }
}

impl PackageRecord {
    /// A simple helper method that constructs a `PackageRecord` with the bare
    /// minimum values.
    pub fn new(name: PackageName, version: impl Into<VersionWithSource>, build: String) -> Self {
        Self {
            arch: None,
            build,
            build_number: 0,
            constrains: vec![],
            depends: vec![],
            features: None,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            md5: None,
            name,
            noarch: NoArchType::default(),
            platform: None,
            python_site_packages_path: None,
            sha256: None,
            size: None,
            subdir: Platform::current().to_string(),
            timestamp: None,
            track_features: vec![],
            version: version.into(),
            purls: None,
            run_exports: None,
        }
    }

    /// Sorts the records topologically.
    ///
    /// This function is deterministic, meaning that it will return the same
    /// result regardless of the order of `records` and of the `depends`
    /// vector inside the records.
    ///
    /// Note that this function only works for packages with unique names.
    pub fn sort_topologically<T: AsRef<PackageRecord> + Clone>(records: Vec<T>) -> Vec<T> {
        topological_sort::sort_topologically(records)
    }

    /// Validate that the given package records are valid w.r.t. 'depends' and
    /// 'constrains'. This function will return Ok(()) if all records form a
    /// valid environment, i.e., all dependencies of each package are
    /// satisfied by the other packages in the list. If there is a
    /// dependency that is not satisfied, this function will return an error.
    pub fn validate<T: AsRef<PackageRecord>>(
        records: Vec<T>,
    ) -> Result<(), ValidatePackageRecordsError> {
        for package in records.iter() {
            let package = package.as_ref();
            // First we check if all dependencies are in the environment.
            for dep in package.depends.iter() {
                // We ignore virtual packages, e.g. `__unix`.
                if dep.starts_with("__") {
                    continue;
                }
                let dep_spec = MatchSpec::from_str(dep, ParseStrictness::Lenient)?;
                if !records.iter().any(|p| dep_spec.matches(p.as_ref())) {
                    return Err(ValidatePackageRecordsError::DependencyNotInEnvironment {
                        package: package.to_owned(),
                        dependency: dep.to_string(),
                    });
                }
            }

            // Then we check if all constraints are satisfied.
            for constraint in package.constrains.iter() {
                let constraint_spec = MatchSpec::from_str(constraint, ParseStrictness::Lenient)?;
                let matching_package = records
                    .iter()
                    .find(|record| Some(record.as_ref().name.clone()) == constraint_spec.name);
                if matching_package.is_some_and(|p| !constraint_spec.matches(p.as_ref())) {
                    return Err(ValidatePackageRecordsError::PackageConstraintNotSatisfied {
                        package: package.to_owned(),
                        constraint: constraint.to_owned(),
                        violating_package: matching_package.unwrap().as_ref().to_owned(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// An error when validating package records.
#[derive(Debug, Error)]
pub enum ValidatePackageRecordsError {
    /// A package is not present in the environment.
    #[error("package '{package}' has dependency '{dependency}', which is not in the environment")]
    DependencyNotInEnvironment {
        /// The package containing the unmet dependency.
        package: PackageRecord,
        /// The dependency that is not in the environment.
        dependency: String,
    },
    /// A package constraint is not met in the environment.
    #[error("package '{package}' has constraint '{constraint}', which is not satisfied by '{violating_package}' in the environment")]
    PackageConstraintNotSatisfied {
        /// The package containing the unmet constraint.
        package: PackageRecord,
        /// The constraint that is violated.
        constraint: String,
        /// The corresponding package that violates the constraint.
        violating_package: PackageRecord,
    },
    /// Failed to parse a matchspec.
    #[error(transparent)]
    ParseMatchSpec(#[from] ParseMatchSpecError),
}

/// An error that can occur when parsing a platform from a string.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum ConvertSubdirError {
    /// No known combination for this platform is known
    #[error("platform: {platform}, arch: {arch} is not a known combination")]
    NoKnownCombination {
        /// The platform string that could not be parsed.
        platform: String,
        /// The architecture.
        arch: String,
    },
    /// Platform key is empty
    #[error("platform key is empty in index.json")]
    PlatformEmpty,
    /// Arch key is empty
    #[error("arch key is empty in index.json")]
    ArchEmpty,
}

/// Determine the subdir based on result taken from the prefix.dev
/// database
/// These were the combinations that have been found in the database.
/// and have been represented in the function.
///
/// # Why can we not use `Platform::FromStr`?
///
/// We cannot use the [`Platform`] `FromStr` directly because `x86` and `x86_64`
/// are different architecture strings. Also some combinations have been
/// removed, because they have not been found.
fn determine_subdir(
    platform: Option<String>,
    arch: Option<String>,
) -> Result<String, ConvertSubdirError> {
    let platform = platform.ok_or(ConvertSubdirError::PlatformEmpty)?;
    let arch = arch.ok_or(ConvertSubdirError::ArchEmpty)?;

    let plat = match platform.as_ref() {
        "linux" => match arch.as_ref() {
            "x86" => Ok(Platform::Linux32),
            "x86_64" => Ok(Platform::Linux64),
            "aarch64" => Ok(Platform::LinuxAarch64),
            "armv61" => Ok(Platform::LinuxArmV6l),
            "armv71" => Ok(Platform::LinuxArmV7l),
            "ppc64le" => Ok(Platform::LinuxPpc64le),
            "ppc64" => Ok(Platform::LinuxPpc64),
            "s390x" => Ok(Platform::LinuxS390X),
            _ => Err(ConvertSubdirError::NoKnownCombination { platform, arch }),
        },
        "osx" => match arch.as_ref() {
            "x86_64" => Ok(Platform::Osx64),
            "arm64" => Ok(Platform::OsxArm64),
            _ => Err(ConvertSubdirError::NoKnownCombination { platform, arch }),
        },
        "windows" => match arch.as_ref() {
            "x86" => Ok(Platform::Win32),
            "x86_64" => Ok(Platform::Win64),
            "arm64" => Ok(Platform::WinArm64),
            _ => Err(ConvertSubdirError::NoKnownCombination { platform, arch }),
        },
        _ => Err(ConvertSubdirError::NoKnownCombination { platform, arch }),
    }?;
    // Convert back to Platform string which should correspond to known subdirs
    Ok(plat.to_string())
}

impl PackageRecord {
    /// Builds a [`PackageRecord`] from a [`IndexJson`] and optionally a size,
    /// sha256 and md5 hash.
    pub fn from_index_json(
        index: IndexJson,
        size: Option<u64>,
        sha256: Option<Sha256Hash>,
        md5: Option<Md5Hash>,
    ) -> Result<PackageRecord, ConvertSubdirError> {
        // Determine the subdir if it can't be found
        let subdir = match index.subdir {
            None => determine_subdir(index.platform.clone(), index.arch.clone())?,
            Some(s) => s,
        };

        Ok(PackageRecord {
            arch: index.arch,
            build: index.build,
            build_number: index.build_number,
            constrains: index.constrains,
            depends: index.depends,
            features: index.features,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: index.license,
            license_family: index.license_family,
            md5,
            name: index.name,
            noarch: index.noarch,
            platform: index.platform,
            python_site_packages_path: index.python_site_packages_path,
            sha256,
            size,
            subdir,
            timestamp: index.timestamp,
            track_features: index.track_features,
            version: index.version,
            purls: None,
            run_exports: None,
        })
    }
}

fn sort_set_alphabetically<S: serde::Serializer>(
    value: &FxHashSet<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.iter().collect::<BTreeSet<_>>().serialize(serializer)
}

#[cfg(test)]
mod test {
    use fxhash::FxHashMap;

    use crate::{
        repo_data::{compute_package_url, determine_subdir},
        Channel, ChannelConfig, PackageRecord, RepoData,
    };

    // isl-0.12.2-1.tar.bz2
    // gmp-5.1.2-6.tar.bz2
    // Are both package variants in the osx-64 subdir
    // Will just test for this case
    #[test]
    fn test_determine_subdir() {
        assert_eq!(
            determine_subdir(Some("osx".to_string()), Some("x86_64".to_string())).unwrap(),
            "osx-64"
        );
    }

    #[test]
    fn test_serialize() {
        let repodata = RepoData {
            version: Some(2),
            info: None,
            packages: FxHashMap::default(),
            conda_packages: FxHashMap::default(),
            removed: ["xyz", "foo", "bar", "baz", "qux", "aux", "quux"]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        };
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_serialize_packages() {
        let repodata = deserialize_json_from_test_data("channels/dummy/linux-64/repodata.json");
        insta::assert_yaml_snapshot!(repodata);

        // serialize to json
        let json = serde_json::to_string_pretty(&repodata).unwrap();
        insta::assert_snapshot!(json);
    }

    #[test]
    fn test_deserialize_no_packages_conda() {
        let repodata = deserialize_json_from_test_data(
            "channels/dummy-no-conda-packages/linux-64/repodata.json",
        );
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_base_url_packages() {
        // load test data
        let test_data_path = dunce::canonicalize(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data"),
        )
        .unwrap();
        let data_path = test_data_path.join("channels/dummy/linux-64/repodata.json");
        let repodata = RepoData::from_path(&data_path).unwrap();

        let channel = Channel::from_str(
            url::Url::from_directory_path(data_path.parent().unwrap().parent().unwrap())
                .unwrap()
                .as_str(),
            &ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap()),
        )
        .unwrap();

        let file_urls = repodata
            .into_repo_data_records(&channel)
            .into_iter()
            .map(|r| {
                pathdiff::diff_paths(r.url.to_file_path().unwrap(), &test_data_path)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();

        // serialize to yaml
        insta::assert_yaml_snapshot!(file_urls);
    }

    #[test]
    fn test_base_url() {
        let channel = Channel::from_str(
            "conda-forge",
            &ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap()),
        )
        .unwrap();
        let base_url = channel.base_url().join("linux-64/").unwrap();
        assert_eq!(
            compute_package_url(&base_url, None, "bla.conda").to_string(),
            "https://conda.anaconda.org/conda-forge/linux-64/bla.conda"
        );
        assert_eq!(
            compute_package_url(&base_url, Some("https://host.some.org"), "bla.conda",).to_string(),
            "https://host.some.org/bla.conda"
        );
        assert_eq!(
            compute_package_url(&base_url, Some("/root"), "bla.conda").to_string(),
            "https://conda.anaconda.org/root/bla.conda"
        );
        assert_eq!(
            compute_package_url(&base_url, Some("foo/bar"), "bla.conda").to_string(),
            "https://conda.anaconda.org/conda-forge/linux-64/foo/bar/bla.conda"
        );
        assert_eq!(
            compute_package_url(&base_url, Some("../../root"), "bla.conda").to_string(),
            "https://conda.anaconda.org/root/bla.conda"
        );
    }

    fn deserialize_json_from_test_data(path: &str) -> RepoData {
        let test_data_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
        let data_path = test_data_path.join(path);
        RepoData::from_path(data_path).unwrap()
    }

    #[test]
    fn test_validate() {
        // load test data
        let test_data_path = dunce::canonicalize(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data"),
        )
        .unwrap();
        let data_path = test_data_path.join("channels/dummy/linux-64/repodata.json");
        let repodata = RepoData::from_path(&data_path).unwrap();

        let package_depends_only_virtual_package = repodata
            .packages
            .get("baz-1.0-unix_py36h1af98f8_2.tar.bz2")
            .unwrap();
        let package_depends = repodata.packages.get("foobar-2.0-bla_1.tar.bz2").unwrap();
        let package_constrains = repodata
            .packages
            .get("foo-3.0.2-py36h1af98f8_3.conda")
            .unwrap();
        let package_bors_1 = repodata.packages.get("bors-1.2.1-bla_1.tar.bz2").unwrap();
        let package_bors_2 = repodata.packages.get("bors-2.1-bla_1.tar.bz2").unwrap();

        assert!(PackageRecord::validate(vec![package_depends_only_virtual_package]).is_ok());
        for packages in [vec![package_depends], vec![package_depends, package_bors_2]] {
            let result = PackageRecord::validate(packages);
            assert!(result.is_err());
            assert!(result.err().unwrap().to_string().contains(
                "package 'foobar=2.0=bla_1' has dependency 'bors <2.0', which is not in the environment"
            ));
        }

        assert!(PackageRecord::validate(vec![package_depends, package_bors_1]).is_ok());
        assert!(PackageRecord::validate(vec![package_constrains]).is_ok());
        assert!(PackageRecord::validate(vec![package_constrains, package_bors_1]).is_ok());

        let result = PackageRecord::validate(vec![package_constrains, package_bors_2]);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains(
            "package 'foo=3.0.2=py36h1af98f8_3' has constraint 'bors <2.0', which is not satisfied by 'bors=2.1=bla_1' in the environment"
        ));
    }
}
