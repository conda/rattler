//! Defines [`RepoData`]. `RepoData` stores information of all packages present
//! in a subdirectory of a channel. It provides indexing functionality.

pub mod patches;
pub mod sharded;
mod topological_sort;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{Display, Formatter},
    hash::{Hash, Hasher},
    path::Path,
    str::FromStr,
};

use indexmap::IndexMap;
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use rattler_macros::sorted;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, DeserializeFromStr, SerializeDisplay};
use thiserror::Error;
use url::Url;

use crate::{
    build_spec::BuildNumber,
    package::{DistArchiveIdentifier, IndexJson, RunExportsJson},
    utils::{
        serde::{
            sort_index_map_alphabetically, sort_map_alphabetically, DeserializeFromStrUnchecked,
        },
        UrlWithTrailingSlash,
    },
    Arch, Channel, MatchSpec, Matches, NoArchType, PackageName, PackageUrl, ParseMatchSpecError,
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
    #[serde(default, serialize_with = "sort_index_map_alphabetically")]
    pub packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,

    /// The conda packages contained in the repodata.json file (under a
    /// different key for backwards compatibility with previous conda
    /// versions)
    #[serde(
        default,
        rename = "packages.conda",
        serialize_with = "sort_index_map_alphabetically"
    )]
    pub conda_packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,

    /// The wheel packages contained in the repodata.json file
    #[serde(
        default,
        rename = "packages.whl",
        serialize_with = "sort_index_map_alphabetically"
    )]
    pub experimental_whl_packages:
        IndexMap<DistArchiveIdentifier, WhlPackageRecord, ahash::RandomState>,

    /// removed packages (files are still accessible, but they are not
    /// installable like regular packages)
    #[serde(
        default,
        serialize_with = "sort_set_alphabetically",
        skip_serializing_if = "ahash::HashSet::is_empty"
    )]
    pub removed: ahash::HashSet<DistArchiveIdentifier>,

    /// The version of the repodata format
    #[serde(rename = "repodata_version")]
    pub version: Option<u64>,
}

/// Information about subdirectory of channel in the Conda [`RepoData`]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct ChannelInfo {
    /// The channel's subdirectory
    pub subdir: Option<String>,

    /// The `base_url` for all package urls. Can be an absolute or relative url.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Trait to allow for generic deserialization of records from a path.
pub trait RecordFromPath {
    /// Deserialize a record from a path.
    fn from_path(path: &Path) -> Result<Self, std::io::Error>
    where
        Self: Sized;
}

/// A single record in the Conda repodata. A single record refers to a single
/// binary distribution of a package on a Conda channel.
#[serde_as]
#[skip_serializing_none]
#[sorted]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Hash)]
pub struct PackageRecord {
    /// Optionally the architecture the package supports. This is almost
    /// always the second part of the `subdir` string. Except for `64` which
    /// maps to `x86_64` and `32` which maps to `x86`. This will be `None` if
    /// the package is `noarch`.
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

    /// Specifications of optional or dependencies. These are dependencies that
    /// are only required if certain features are enabled or if certain
    /// conditions are met.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(rename = "extra_depends")]
    pub experimental_extra_depends: BTreeMap<String, Vec<String>>,

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

    /// Optionally the platform the package supports.
    /// Note that this does not match the [`Platform`] enum, but is only the
    /// first part of the platform (e.g. `linux`, `osx`, `win`, ...).
    /// The `subdir` field contains the `Platform` enum.
    pub platform: Option<String>,

    /// Package identifiers of packages that are equivalent to this package but
    /// from other ecosystems.
    /// starting from 0.23.2, this field became [`Option<Vec<PackageUrl>>`].
    /// This was done to support older lockfiles,
    /// where we didn't differentiate between empty purl and missing one.
    /// Now, `None::` means that the purl is missing, and it will be tried to
    /// filled in. So later it can be one of the following:
    /// [`Some(vec![])`] means that the purl is empty and package is not pypi
    /// one. [`Some([`PackageUrl`])`] means that it is a pypi package.
    /// See this CEP: <https://github.com/conda/ceps/pull/63>
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
    pub timestamp: Option<crate::utils::TimestampMs>,

    /// Track features are nowadays only used to downweight packages (ie. give
    /// them less priority). To that effect, the package is downweighted
    /// by the number of `track_features`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "crate::utils::serde::Features")]
    pub track_features: Vec<String>,

    /// The version of the package
    pub version: VersionWithSource,
    // Looking at the `PackageRecord` class in the Conda source code a record can also include all
    // these fields. However, I have no idea if or how they are used so I left them out.
    //pub preferred_env: Option<String>,
    //pub date: Option<String>,
    //pub package_type: ?
}

impl PartialOrd for PackageRecord {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageRecord {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| {
                // Packages with tracked features are sorted after packages
                // without tracked features.
                self.track_features
                    .is_empty()
                    .cmp(&other.track_features.is_empty())
                    .reverse()
            })
            .then_with(|| self.version.cmp(&other.version).reverse())
            .then_with(|| self.build_number.cmp(&other.build_number).reverse())
            .then_with(|| self.timestamp.cmp(&other.timestamp).reverse())
    }
}

/// A record in the `packages.whl` section of the `repodata.json`.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Hash)]
pub struct WhlPackageRecord {
    /// The conda metadata
    #[serde(flatten)]
    pub package_record: PackageRecord,

    /// Where to get the package from. This is a required field.
    pub url: UrlOrPath,
}

impl AsRef<PackageRecord> for WhlPackageRecord {
    fn as_ref(&self) -> &PackageRecord {
        self.package_record.as_ref()
    }
}

/// Represents either an absolute URL or a relative path to the base url of a
/// channel
#[derive(Debug, DeserializeFromStr, SerializeDisplay, Eq, PartialEq, Clone)]
pub enum UrlOrPath {
    /// A relative path to the base url of the channel
    Path(String),

    /// An absolute URL
    Url(Url),
}

impl UrlOrPath {
    /// Returns the string representation of the URL or path.
    pub fn as_str(&self) -> &str {
        match self {
            UrlOrPath::Path(path) => path,
            UrlOrPath::Url(url) => url.as_str(),
        }
    }
}

impl Hash for UrlOrPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl Display for UrlOrPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlOrPath::Path(path) => write!(f, "{path}"),
            UrlOrPath::Url(url) => write!(f, "{url}"),
        }
    }
}

impl FromStr for UrlOrPath {
    type Err = url::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First try to parse the string as a path.
        if s.contains("://") {
            Ok(UrlOrPath::Url(s.parse()?))
        } else {
            Ok(UrlOrPath::Path(s.to_owned()))
        }
    }
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

impl RecordFromPath for PackageRecord {
    fn from_path(path: &Path) -> Result<Self, std::io::Error> {
        let contents = fs_err::read_to_string(path)?;
        Ok(serde_json::from_str(&contents)?)
    }
}

impl PackageRecord {
    /// Returns true if package `run_exports` is some.
    pub fn has_run_exports(&self) -> bool {
        self.run_exports.is_some()
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
        let base_url = self.base_url().map(ToOwned::to_owned);

        // Determine the base_url of the channel
        for (identifier, package_record) in self.packages.into_iter().chain(self.conda_packages) {
            records.push(RepoDataRecord {
                url: compute_package_url(
                    &channel
                        .base_url
                        .url()
                        .join(&package_record.subdir)
                        .expect("cannot join channel base_url and subdir"),
                    base_url.as_deref(),
                    &identifier.to_file_name(),
                ),
                channel: Some(channel.base_url.as_str().to_string()),
                package_record,
                identifier,
            });
        }

        // Determine the base_url of the channel
        for (
            identifier,
            WhlPackageRecord {
                url,
                package_record,
            },
        ) in self.experimental_whl_packages
        {
            let url = match url {
                UrlOrPath::Path(path) => compute_package_url(
                    &channel
                        .base_url
                        .url()
                        .join(&package_record.subdir)
                        .expect("cannot join channel base_url and subdir"),
                    base_url.as_deref(),
                    &path,
                ),
                UrlOrPath::Url(url) => url,
            };

            records.push(RepoDataRecord {
                url,
                channel: Some(channel.base_url.as_str().to_string()),
                package_record,
                identifier,
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
                UrlWithTrailingSlash::from(repo_data_base_url.clone())
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
            experimental_extra_depends: BTreeMap::new(),
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
    ) -> Result<(), Box<ValidatePackageRecordsError>> {
        for package in records.iter() {
            let package = package.as_ref();
            // First we check if all dependencies are in the environment.
            for dep in package.depends.iter() {
                // We ignore virtual packages, e.g. `__unix`.
                if dep.starts_with("__") {
                    continue;
                }
                let dep_spec = MatchSpec::from_str(dep, ParseStrictness::Lenient)
                    .map_err(ValidatePackageRecordsError::ParseMatchSpec)?;
                if !records.iter().any(|p| dep_spec.matches(p.as_ref())) {
                    return Err(Box::new(
                        ValidatePackageRecordsError::DependencyNotInEnvironment {
                            package: package.to_owned(),
                            dependency: dep.clone(),
                        },
                    ));
                }
            }

            // Then we check if all constraints are satisfied.
            for constraint in package.constrains.iter() {
                let constraint_spec = MatchSpec::from_str(constraint, ParseStrictness::Lenient)
                    .map_err(ValidatePackageRecordsError::ParseMatchSpec)?;
                let matching_package = records.iter().find(|record| match &constraint_spec.name {
                    Some(matcher) => matcher.matches(&record.as_ref().name),
                    None => false,
                });
                if matching_package.is_some_and(|p| !constraint_spec.matches(p.as_ref())) {
                    return Err(Box::new(
                        ValidatePackageRecordsError::PackageConstraintNotSatisfied {
                            package: package.to_owned(),
                            constraint: constraint.to_owned(),
                            violating_package: matching_package.unwrap().as_ref().to_owned(),
                        },
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Eq, PartialEq, Clone)]
struct PackageRunExports {
    run_exports: RunExportsJson,
}

/// Represents [`Channel`] global map from package file names to
/// [`RunExportsJson`].
///
/// See [CEP 12](https://github.com/conda/ceps/blob/main/cep-0012.md) for more info.
#[derive(Debug, Default, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct SubdirRunExportsJson {
    info: Option<ChannelInfo>,

    #[serde(default, serialize_with = "sort_map_alphabetically")]
    packages: ahash::HashMap<DistArchiveIdentifier, PackageRunExports>,

    #[serde(
        default,
        rename = "packages.conda",
        serialize_with = "sort_map_alphabetically"
    )]
    conda_packages: ahash::HashMap<DistArchiveIdentifier, PackageRunExports>,
}

impl SubdirRunExportsJson {
    /// Get package [`RunExportsJson`] based on the package file name.
    pub fn get(&self, record: &RepoDataRecord) -> Option<&RunExportsJson> {
        let file_name = &record.identifier;
        self.packages
            .get(file_name)
            .or_else(|| self.conda_packages.get(file_name))
            .map(|pre| &pre.run_exports)
    }

    /// Returns optional [`ChannelInfo`].
    pub fn info(&self) -> Option<&ChannelInfo> {
        self.info.as_ref()
    }
}

/// An error when validating package records.
#[derive(Debug, Error)]
#[allow(clippy::large_enum_variant)]
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
    #[error("package '{package}' has constraint '{constraint}', which is not satisfied by '{violating_package}' in the environment"
    )]
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

    match arch.parse::<Arch>() {
        Ok(arch) => {
            let arch_str = match arch {
                Arch::X86 => "32",
                Arch::X86_64 => "64",
                _ => arch.as_str(),
            };
            Ok(format!("{platform}-{arch_str}"))
        }
        Err(_) => Err(ConvertSubdirError::NoKnownCombination { platform, arch }),
    }
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
            experimental_extra_depends: index.experimental_extra_depends,
            sha256,
            size,
            subdir,
            timestamp: index.timestamp,
            track_features: index.track_features,
            version: index.version,
            purls: index.purls,
            run_exports: None,
        })
    }
}

fn sort_set_alphabetically<K: Ord + Serialize, S: serde::Serializer>(
    value: &ahash::HashSet<K>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.iter().collect::<BTreeSet<_>>().serialize(serializer)
}

#[cfg(test)]
mod test {
    use indexmap::IndexMap;

    use crate::{
        package::DistArchiveIdentifier,
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
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            experimental_whl_packages: IndexMap::default(),
            removed: [
                "xyz-1-py.conda",
                "foo-1-py.conda",
                "bar-1-py.conda",
                "baz-1-py.conda",
                "qux-1-py.tar.bz2",
                "aux-1-py.tar.bz2",
                "quux-1-py.conda",
            ]
            .iter()
            .map(|s| DistArchiveIdentifier::try_from_filename(s).unwrap())
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
    fn test_deserialize_no_noarch_empty_str() {
        // This test covers the case where a repodata entry may contain a "noarch" key
        // set to an empty string. Packages with such metadata have been
        // observed on private conda channels. This likely was passed from older
        // versions of conda-build that would pass this key from the recipe even
        // if it was incorrect.
        let repodata =
            deserialize_json_from_test_data("channels/dummy-noarch-str/linux-64/repodata.json");
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_deserialize_no_noarch_not_empty_str_should_fail() {
        let test_data_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
        let data_path =
            test_data_path.join("channels/dummy-noarch-str-not-empty/linux-64/repodata.json");
        let err = RepoData::from_path(data_path).unwrap_err();
        insta::assert_snapshot!(err.to_string(), @r###"invalid value: string "notempty-this-should-fail", expected '' at line 26 column 43"###);
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
        let base_url = channel.base_url.url().join("linux-64/").unwrap();
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
            .get(
                &DistArchiveIdentifier::try_from_filename("baz-1.0-unix_py36h1af98f8_2.tar.bz2")
                    .unwrap(),
            )
            .unwrap();
        let package_depends = repodata
            .packages
            .get(&DistArchiveIdentifier::try_from_filename("foobar-2.0-bla_1.tar.bz2").unwrap())
            .unwrap();
        let package_constrains = repodata
            .packages
            .get(
                &DistArchiveIdentifier::try_from_filename("foo-3.0.2-py36h1af98f8_3.conda")
                    .unwrap(),
            )
            .unwrap();
        let package_bors_1 = repodata
            .packages
            .get(&DistArchiveIdentifier::try_from_filename("bors-1.2.1-bla_1.tar.bz2").unwrap())
            .unwrap();
        let package_bors_2 = repodata
            .packages
            .get(&DistArchiveIdentifier::try_from_filename("bors-2.1-bla_1.tar.bz2").unwrap())
            .unwrap();

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

    #[test]
    fn test_packages_serialized_alphabetically() {
        use crate::{PackageName, Version};

        // Create a RepoData with packages inserted in NON-alphabetical order
        let mut packages = IndexMap::default();
        let mut conda_packages = IndexMap::default();

        // Insert packages in deliberately non-alphabetical order: z, a, m, b
        packages.insert(
            "zebra-1.0-h123.tar.bz2".parse().unwrap(),
            PackageRecord::new(
                PackageName::new_unchecked("zebra"),
                Version::major(1),
                "h123".to_string(),
            ),
        );
        packages.insert(
            "apple-2.0-h456.tar.bz2".parse().unwrap(),
            PackageRecord::new(
                PackageName::new_unchecked("apple"),
                Version::major(2),
                "h456".to_string(),
            ),
        );
        packages.insert(
            "mango-1.5-h789.tar.bz2".parse().unwrap(),
            PackageRecord::new(
                PackageName::new_unchecked("mango"),
                Version::major(1),
                "h789".to_string(),
            ),
        );
        packages.insert(
            "banana-3.0-habc.tar.bz2".parse().unwrap(),
            PackageRecord::new(
                PackageName::new_unchecked("banana"),
                Version::major(3),
                "habc".to_string(),
            ),
        );

        // Insert conda packages in non-alphabetical order too
        conda_packages.insert(
            "xray-1.0-h111.conda".parse().unwrap(),
            PackageRecord::new(
                PackageName::new_unchecked("xray"),
                Version::major(1),
                "h111".to_string(),
            ),
        );
        conda_packages.insert(
            "alpha-2.0-h222.conda".parse().unwrap(),
            PackageRecord::new(
                PackageName::new_unchecked("alpha"),
                Version::major(2),
                "h222".to_string(),
            ),
        );
        conda_packages.insert(
            "omega-3.0-h333.conda".parse().unwrap(),
            PackageRecord::new(
                PackageName::new_unchecked("omega"),
                Version::major(3),
                "h333".to_string(),
            ),
        );

        let repodata = RepoData {
            version: Some(2),
            info: None,
            packages,
            conda_packages,
            experimental_whl_packages: IndexMap::default(),
            removed: ahash::HashSet::default(),
        };

        // Serialize to JSON string
        let json = serde_json::to_string(&repodata).unwrap();

        // Parse the JSON to extract the package keys
        let json_value: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Check that packages are in alphabetical order
        if let Some(packages) = json_value.get("packages").and_then(|p| p.as_object()) {
            let keys: Vec<&String> = packages.keys().collect();
            let mut sorted_keys = keys.clone();
            sorted_keys.sort();
            assert_eq!(
                keys, sorted_keys,
                "packages should be serialized in alphabetical order"
            );
        }

        // Check that packages.conda are in alphabetical order
        if let Some(conda_packages) = json_value.get("packages.conda").and_then(|p| p.as_object()) {
            let keys: Vec<&String> = conda_packages.keys().collect();
            let mut sorted_keys = keys.clone();
            sorted_keys.sort();
            assert_eq!(
                keys, sorted_keys,
                "packages.conda should be serialized in alphabetical order"
            );
        }
    }
}
