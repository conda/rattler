//! Defines [`RepoData`]. `RepoData` stores information of all packages present
//! in a subdirectory of a channel. It provides indexing functionality.

pub mod patches;
pub mod sharded;
mod topological_sort;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{Display, Formatter},
    hash::{Hash, Hasher},
    num::ParseIntError,
    path::Path,
    str::FromStr,
};

use indexmap::IndexMap;
use rattler_digest::{Md5Hash, Sha256Hash, serde::SerializableHash};
use rattler_macros::sorted;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as DeError};
use serde_with::{
    DeserializeFromStr, DisplayFromStr, SerializeDisplay, serde_as, skip_serializing_none,
};
use thiserror::Error;
use url::Url;

use crate::{
    Arch, Channel, Flag, MatchSpec, Matches, NoArchType, PackageName, PackageUrl,
    ParseMatchSpecError, ParseStrictness, Platform, RepoDataRecord, VersionWithSource,
    build_spec::BuildNumber,
    package::{
        ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier, IndexJson, RunExportsJson,
        WheelArchiveType,
    },
    utils::{
        TimestampMs, UrlWithTrailingSlash,
        serde::{
            DeserializeFromStrUnchecked, sort_index_map_alphabetically, sort_map_alphabetically,
            sort_set_alphabetically,
        },
    },
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

    /// Packages stored under the `v3` top-level key.
    /// Uses extension-less `ArchiveIdentifier` keys with sub-maps for each
    /// archive type.
    #[serde(default, skip_serializing_if = "V3Packages::is_empty")]
    pub v3: V3Packages,

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
#[serde_as]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct ChannelInfo {
    /// The channel's subdirectory
    pub subdir: Option<String>,

    /// The `base_url` for all package urls. Can be an absolute or relative url.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Repodata revisions available in this repodata file.
    ///
    /// Serialized as a `vN`-keyed dictionary per the CEP draft
    /// <https://github.com/conda/ceps/pull/146>.
    #[serde_as(as = "IndexMap<DisplayFromStr, _>")]
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub repodata_revisions: RepodataRevisions,

    /// Optional relationships to other channels as defined in
    /// [CEP-42](https://github.com/conda/ceps/blob/main/cep-0042.md).
    #[serde(default, skip_serializing_if = "ChannelRelations::is_none_or_empty")]
    pub channel_relations: Option<ChannelRelations>,

    /// Virtual package detection plugins registered by the channel.
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[serde_as(
        deserialize_as = "IndexMap<DeserializeFromStrUnchecked, Vec<DeserializeFromStrUnchecked>>"
    )]
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub virtual_package_plugins: VirtualPackagePlugins,
}

/// Virtual package detection plugins registered by a channel: the name of the
/// package providing the plugin, mapped to the virtual packages it provides.
///
/// One plugin may provide several virtual packages, e.g. a `cuda-detect`
/// providing both `__cuda` and `__cuda_arch`. The executable to run is named
/// after the plugin package. Inverting the map is left to the caller: the
/// reverse direction is many-to-many.
#[cfg(feature = "experimental-virtual-package-plugins")]
pub type VirtualPackagePlugins = IndexMap<PackageName, Vec<PackageName>>;

/// Repodata revisions keyed by revision, mirroring the `vN` dictionary of the
/// CEP draft <https://github.com/conda/ceps/pull/146>. Keying encodes
/// uniqueness; insertion order is preserved.
pub type RepodataRevisions = IndexMap<RepodataRevision, RepodataRevisionMetadata>;

/// Metadata for a single [`RepodataRevisions`] entry; the revision itself is
/// the map key.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Default)]
pub struct RepodataRevisionMetadata {
    /// An optional message describing this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// The number of packages available in this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_packages: Option<u64>,

    /// The Unix timestamp in milliseconds of the oldest record in this
    /// revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest: Option<TimestampMs>,

    /// The Unix timestamp in milliseconds of the newest record in this
    /// revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest: Option<TimestampMs>,
}

/// Published metadata for a repodata revision.
///
/// In `info.repodata_revisions`, the revision is represented by the enclosing
/// `vN` map key. This flattened form is useful when revision metadata is
/// handled as an individual value.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct RepodataRevisionInfo {
    /// The integer identifying the revision.
    #[serde(default)]
    pub revision: RepodataRevision,

    /// An optional message describing this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// The number of packages available in this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_packages: Option<u64>,

    /// The oldest package timestamp in this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest: Option<TimestampMs>,

    /// The newest package timestamp in this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest: Option<TimestampMs>,
}

/// Indexer configuration selecting a repodata revision to publish.
///
/// Package counts and timestamps are derived from emitted records, so callers
/// can only select a revision and optionally override its message.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct RepodataRevisionSelection {
    /// The revision to publish.
    #[serde(default)]
    pub revision: RepodataRevision,

    /// An optional publisher message for this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A repodata revision.
///
/// Legacy repodata predates numbered layouts and uses the `packages` and
/// `packages.conda` maps. CEP 48 adds the v3 layout. Other numeric values are
/// preserved so readers can report future revisions without interpreting them.
#[derive(Debug, Default, Eq, PartialEq, Clone, Copy, Hash, Ord, PartialOrd)]
pub enum RepodataRevision {
    /// Repodata using the legacy `packages` and `packages.conda` maps.
    #[default]
    Legacy,
    /// Repodata records stored under the top-level `v3` map.
    V3,
    /// A revision not modeled by rattler.
    Unknown(u64),
}

impl RepodataRevision {
    /// Returns the integer representation used in repodata JSON.
    pub fn as_u64(self) -> u64 {
        match self {
            Self::Legacy => 0,
            Self::V3 => 3,
            Self::Unknown(value) => value,
        }
    }

    /// Returns whether this revision uses the legacy package-map layout.
    pub fn uses_legacy_package_layout(self) -> bool {
        self == Self::Legacy
    }
}

impl From<u64> for RepodataRevision {
    fn from(value: u64) -> Self {
        match value {
            0 => Self::Legacy,
            3 => Self::V3,
            value => Self::Unknown(value),
        }
    }
}

impl From<RepodataRevision> for u64 {
    fn from(value: RepodataRevision) -> Self {
        value.as_u64()
    }
}

impl FromStr for RepodataRevision {
    type Err = ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("legacy") {
            return Ok(Self::Legacy);
        }

        value
            .strip_prefix('v')
            .or_else(|| value.strip_prefix('V'))
            .unwrap_or(value)
            .parse::<u64>()
            .map(Self::from)
    }
}

impl Display for RepodataRevision {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.as_u64())
    }
}

impl Serialize for RepodataRevision {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(self.as_u64())
    }
}

impl<'de> Deserialize<'de> for RepodataRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Self::from)
    }
}

/// Relationships between a channel and other channels as declared in the
/// channel's `repodata.json` (or sharded repodata index) under
/// `info.channel_relations`.
///
/// See [CEP-42](https://github.com/conda/ceps/blob/main/cep-0042.md) for
/// details. Both fields are relative-path channel references (e.g.
/// `../conda-forge`) resolved against the declaring channel's base URL
/// without its subdir component.
///
/// A channel MUST NOT declare both `base` and `overrides` referencing the
/// same channel.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Default)]
pub struct ChannelRelations {
    /// A reference to a channel with higher priority than the declaring
    /// channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,

    /// A reference to a channel with lower priority than the declaring
    /// channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<String>,
}

impl ChannelRelations {
    /// Returns true if neither `base` nor `overrides` is set.
    pub fn is_empty(&self) -> bool {
        self.base.is_none() && self.overrides.is_none()
    }

    pub(crate) fn is_none_or_empty(value: &Option<ChannelRelations>) -> bool {
        value.as_ref().is_none_or(ChannelRelations::is_empty)
    }
}

const RESERVED_V3_BUCKETS: [&str; 3] = ["conda", "tar.bz2", "whl"];

/// An error returned when an extension bucket collides with a typed v3 bucket.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
#[error("'{extension}' is a reserved v3 artifact bucket")]
pub struct ReservedV3ExtensionError {
    extension: String,
}

impl ReservedV3ExtensionError {
    /// Returns the reserved extension that caused this error.
    pub fn extension(&self) -> &str {
        &self.extension
    }
}

/// Extension buckets for v3 artifact types not yet modeled by rattler.
///
/// This type keeps the underlying map private so callers cannot add buckets
/// that collide with the typed `conda`, `tar.bz2`, and `whl` fields.
#[derive(Debug, Serialize, Eq, PartialEq, Clone, Default)]
#[serde(transparent)]
pub struct V3Extensions(BTreeMap<String, serde_json::Value>);

impl V3Extensions {
    /// Returns true if this set contains no extension buckets.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns a bucket by its artifact extension.
    pub fn get(&self, extension: &str) -> Option<&serde_json::Value> {
        self.0.get(extension)
    }

    /// Returns a mutable bucket by its artifact extension.
    pub fn get_mut(&mut self, extension: &str) -> Option<&mut serde_json::Value> {
        self.0.get_mut(extension)
    }

    /// Iterates over artifact extension buckets in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &serde_json::Value)> {
        self.0.iter()
    }

    /// Adds or replaces an extension bucket.
    ///
    /// Typed buckets must be accessed through [`V3Packages`] directly, so this
    /// rejects their reserved names.
    pub fn insert(
        &mut self,
        extension: impl Into<String>,
        bucket: serde_json::Value,
    ) -> Result<Option<serde_json::Value>, ReservedV3ExtensionError> {
        let extension = extension.into();
        if RESERVED_V3_BUCKETS.contains(&extension.as_str()) {
            return Err(ReservedV3ExtensionError { extension });
        }
        Ok(self.0.insert(extension, bucket))
    }

    /// Removes an extension bucket.
    pub fn remove(&mut self, extension: &str) -> Option<serde_json::Value> {
        self.0.remove(extension)
    }
}

impl TryFrom<BTreeMap<String, serde_json::Value>> for V3Extensions {
    type Error = ReservedV3ExtensionError;

    fn try_from(extensions: BTreeMap<String, serde_json::Value>) -> Result<Self, Self::Error> {
        if let Some(extension) = extensions
            .keys()
            .find(|extension| RESERVED_V3_BUCKETS.contains(&extension.as_str()))
        {
            return Err(ReservedV3ExtensionError {
                extension: extension.clone(),
            });
        }
        Ok(Self(extensions))
    }
}

impl<'de> Deserialize<'de> for V3Extensions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        BTreeMap::<String, serde_json::Value>::deserialize(deserializer)
            .and_then(|extensions| Self::try_from(extensions).map_err(D::Error::custom))
    }
}

/// Packages stored under the `v3` top-level key.
///
/// Records in this set of packages can have conditional dependencies, extras
/// and can be whls.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Default)]
pub struct V3Packages {
    /// The tar.bz2 package records
    #[serde(
        default,
        rename = "tar.bz2",
        serialize_with = "sort_map_alphabetically",
        skip_serializing_if = "ahash::HashMap::is_empty"
    )]
    pub tar_bz2: ahash::HashMap<ArchiveIdentifier, PackageRecord>,

    /// The conda package records
    #[serde(
        default,
        serialize_with = "sort_map_alphabetically",
        skip_serializing_if = "ahash::HashMap::is_empty"
    )]
    pub conda: ahash::HashMap<ArchiveIdentifier, PackageRecord>,

    /// The whl package records
    #[serde(
        default,
        serialize_with = "sort_map_alphabetically",
        skip_serializing_if = "ahash::HashMap::is_empty"
    )]
    pub whl: ahash::HashMap<ArchiveIdentifier, WhlPackageRecord>,

    /// Package buckets for archive extensions not yet modeled by rattler.
    ///
    /// These are preserved without interpretation so that readers do not
    /// discard data from newer repodata revisions.
    #[serde(flatten, default, skip_serializing_if = "V3Extensions::is_empty")]
    pub extensions: V3Extensions,
}

impl V3Packages {
    /// Returns true if all sub-maps are empty.
    pub fn is_empty(&self) -> bool {
        self.tar_bz2.is_empty()
            && self.conda.is_empty()
            && self.whl.is_empty()
            && self.extensions.is_empty()
    }

    /// Iterates over all package records from typed v3 buckets with their
    /// archive identifiers.
    ///
    /// Extension buckets remain available through [`Self::extensions`].
    pub fn records(&self) -> impl Iterator<Item = (DistArchiveIdentifier, &PackageRecord)> + '_ {
        self.tar_bz2
            .iter()
            .map(|(id, record)| {
                (
                    DistArchiveIdentifier::new(id.clone(), CondaArchiveType::TarBz2),
                    record,
                )
            })
            .chain(self.conda.iter().map(|(id, record)| {
                (
                    DistArchiveIdentifier::new(id.clone(), CondaArchiveType::Conda),
                    record,
                )
            }))
            .chain(self.whl.iter().map(|(id, record)| {
                (
                    DistArchiveIdentifier::new(id.clone(), WheelArchiveType::Whl),
                    &record.package_record,
                )
            }))
    }

    /// Consumes this value and iterates over typed package records with their
    /// archive identifiers and optional wheel URL.
    ///
    /// Unknown extension buckets are not represented by this typed iterator.
    /// Transformations that must preserve them should use
    /// [`Self::into_records_with_url_and_extensions`] instead.
    pub fn into_records_with_url(
        self,
    ) -> impl Iterator<Item = (DistArchiveIdentifier, PackageRecord, Option<UrlOrPath>)> {
        self.into_records_with_url_and_extensions().0
    }

    /// Consumes this value and returns typed package records with their archive
    /// identifiers and optional wheel URL, together with untyped extension
    /// buckets.
    ///
    /// Use this method for transformations that need to preserve future v3
    /// artifact types.
    pub fn into_records_with_url_and_extensions(
        self,
    ) -> (
        impl Iterator<Item = (DistArchiveIdentifier, PackageRecord, Option<UrlOrPath>)>,
        V3Extensions,
    ) {
        let extensions = self.extensions;
        (
            self.tar_bz2
                .into_iter()
                .map(|(id, record)| {
                    (
                        DistArchiveIdentifier::new(id, CondaArchiveType::TarBz2),
                        record,
                        None,
                    )
                })
                .chain(self.conda.into_iter().map(|(id, record)| {
                    (
                        DistArchiveIdentifier::new(id, CondaArchiveType::Conda),
                        record,
                        None,
                    )
                }))
                .chain(self.whl.into_iter().map(|(id, record)| {
                    (
                        DistArchiveIdentifier::new(id, WheelArchiveType::Whl),
                        record.package_record,
                        Some(record.url),
                    )
                })),
            extensions,
        )
    }
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
    pub extra_depends: BTreeMap<String, Vec<String>>,

    /// Features are a deprecated way to specify different feature sets for the
    /// conda solver. This is not supported anymore and should not be used.
    /// Instead, `mutex` packages should be used to specify
    /// mutually exclusive features.
    pub features: Option<String>,

    /// Plain string flags used to select package variants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<Flag>,

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
                // Packages with tracked features are sorted before packages
                // without tracked features.
                self.track_features
                    .is_empty()
                    .cmp(&other.track_features.is_empty())
            })
            .then_with(|| self.version.cmp(&other.version))
            .then_with(|| self.build_number.cmp(&other.build_number))
            .then_with(|| self.timestamp.cmp(&other.timestamp))
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

    /// Returns the timestamp used by indexing operations.
    ///
    /// This currently returns the package build timestamp. A future index
    /// timestamp can change this method without changing its callers.
    pub fn timestamp_for_indexing(&self) -> Option<TimestampMs> {
        self.timestamp
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
        let channel_str = channel.base_url.as_str().to_string();

        let subdir_url = |subdir: &str| {
            channel
                .base_url
                .url()
                .join(subdir)
                .expect("cannot join channel base_url and subdir")
        };

        // Conda packages: packages, packages.conda, v3.tar.bz2, v3.conda
        let v3_tar_bz2 = self.v3.tar_bz2.into_iter().map(|(id, rec)| {
            (
                DistArchiveIdentifier::new(id, CondaArchiveType::TarBz2),
                rec,
            )
        });
        let v3_conda = self
            .v3
            .conda
            .into_iter()
            .map(|(id, rec)| (DistArchiveIdentifier::new(id, CondaArchiveType::Conda), rec));

        for (identifier, package_record) in self
            .packages
            .into_iter()
            .chain(self.conda_packages)
            .chain(v3_tar_bz2)
            .chain(v3_conda)
        {
            records.push(RepoDataRecord {
                url: compute_package_url(
                    &subdir_url(&package_record.subdir),
                    base_url.as_deref(),
                    &identifier.to_file_name(),
                ),
                channel: Some(channel_str.clone()),
                package_record,
                identifier,
            });
        }

        // Whl packages: v3.whl
        for (
            id,
            WhlPackageRecord {
                url,
                package_record,
            },
        ) in self.v3.whl
        {
            let dist_id = DistArchiveIdentifier::new(id, WheelArchiveType::Whl);
            let url = match url {
                UrlOrPath::Path(path) => compute_package_url(
                    &subdir_url(&package_record.subdir),
                    base_url.as_deref(),
                    &path,
                ),
                UrlOrPath::Url(url) => url,
            };

            records.push(RepoDataRecord {
                url,
                channel: Some(channel_str.clone()),
                package_record,
                identifier: dist_id,
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
            flags: vec![],
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            md5: None,
            name,
            noarch: NoArchType::default(),
            platform: None,
            python_site_packages_path: None,
            extra_depends: BTreeMap::new(),
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
                let matching_package = records
                    .iter()
                    .find(|record| constraint_spec.name.matches(&record.as_ref().name));
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

    /// Run exports for v3 packages.
    #[serde(default, skip_serializing_if = "V3RunExports::is_empty")]
    v3: V3RunExports,
}

/// Run exports for packages stored under the `v3` top-level key.
#[derive(Debug, Default, Deserialize, Serialize, Eq, PartialEq, Clone)]
struct V3RunExports {
    /// Run exports for v3 tar.bz2 packages
    #[serde(
        default,
        rename = "tar.bz2",
        serialize_with = "sort_map_alphabetically",
        skip_serializing_if = "ahash::HashMap::is_empty"
    )]
    tar_bz2: ahash::HashMap<ArchiveIdentifier, PackageRunExports>,

    /// Run exports for v3 conda packages
    #[serde(
        default,
        serialize_with = "sort_map_alphabetically",
        skip_serializing_if = "ahash::HashMap::is_empty"
    )]
    conda: ahash::HashMap<ArchiveIdentifier, PackageRunExports>,
}

impl V3RunExports {
    /// Returns true if all sub-maps are empty.
    pub fn is_empty(&self) -> bool {
        self.tar_bz2.is_empty() && self.conda.is_empty()
    }
}

impl SubdirRunExportsJson {
    /// Get package [`RunExportsJson`] based on the package file name.
    pub fn get(&self, record: &RepoDataRecord) -> Option<&RunExportsJson> {
        let file_name = &record.identifier;
        self.packages
            .get(file_name)
            .or_else(|| self.conda_packages.get(file_name))
            .or_else(|| {
                self.v3
                    .tar_bz2
                    .get(&file_name.identifier)
                    .or_else(|| self.v3.conda.get(&file_name.identifier))
            })
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
    #[error(
        "package '{package}' has constraint '{constraint}', which is not satisfied by '{violating_package}' in the environment"
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
            flags: index.flags,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: index.license,
            license_family: index.license_family,
            md5,
            name: index.name,
            noarch: index.noarch,
            platform: index.platform,
            python_site_packages_path: index.python_site_packages_path,
            extra_depends: index.extra_depends,
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

#[cfg(test)]
mod test {
    use indexmap::IndexMap;

    use crate::{
        Channel, ChannelConfig, ChannelInfo, ChannelRelations, PackageRecord, RepoData,
        RepodataRevision, V3Extensions, V3Packages,
        package::DistArchiveIdentifier,
        repo_data::{compute_package_url, determine_subdir},
    };
    #[cfg(feature = "experimental-virtual-package-plugins")]
    use crate::{PackageName, repo_data::VirtualPackagePlugins};

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
            v3: V3Packages::default(),
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

    // See https://github.com/conda/ceps/blob/main/cep-0042.md
    #[test]
    fn test_channel_relations() {
        // Deserialize a repodata.json with channel_relations set.
        let raw = r#"{
            "info": {
                "subdir": "linux-64",
                "channel_relations": {
                    "base": "../conda-forge",
                    "overrides": "../fallback-channel"
                }
            },
            "packages": {},
            "packages.conda": {}
        }"#;
        let repodata: RepoData = serde_json::from_str(raw).unwrap();
        let relations = repodata
            .info
            .as_ref()
            .and_then(|i| i.channel_relations.as_ref())
            .unwrap();
        assert_eq!(relations.base.as_deref(), Some("../conda-forge"));
        assert_eq!(relations.overrides.as_deref(), Some("../fallback-channel"));

        // Round trip with a single field set and the other omitted.
        let partial = RepoData {
            version: Some(2),
            info: Some(ChannelInfo {
                subdir: Some("linux-64".to_string()),
                base_url: None,
                repodata_revisions: IndexMap::default(),
                channel_relations: Some(ChannelRelations {
                    base: Some("../conda-forge".to_string()),
                    overrides: None,
                }),
                #[cfg(feature = "experimental-virtual-package-plugins")]
                virtual_package_plugins: VirtualPackagePlugins::default(),
            }),
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: ahash::HashSet::default(),
        };
        let json = serde_json::to_string(&partial).unwrap();
        assert!(json.contains("\"channel_relations\""));
        assert!(json.contains("\"base\":\"../conda-forge\""));
        assert!(!json.contains("\"overrides\""));
        assert_eq!(serde_json::from_str::<RepoData>(&json).unwrap(), partial);

        // `channel_relations` must be omitted when it is `None` as well as
        // when both of its fields are unset.
        for channel_relations in [None, Some(ChannelRelations::default())] {
            let repodata = RepoData {
                version: Some(2),
                info: Some(ChannelInfo {
                    subdir: Some("linux-64".to_string()),
                    base_url: None,
                    repodata_revisions: IndexMap::default(),
                    channel_relations,
                    #[cfg(feature = "experimental-virtual-package-plugins")]
                    virtual_package_plugins: VirtualPackagePlugins::default(),
                }),
                packages: IndexMap::default(),
                conda_packages: IndexMap::default(),
                v3: V3Packages::default(),
                removed: ahash::HashSet::default(),
            };
            let json = serde_json::to_string(&repodata).unwrap();
            assert!(!json.contains("channel_relations"));
        }
    }

    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[test]
    fn test_virtual_package_plugins() {
        // A single plugin may provide several virtual packages; the order of
        // both the plugins and their virtual packages is preserved.
        let raw = r#"{
            "info": {
                "subdir": "linux-64",
                "virtual_package_plugins": {
                    "cuda-detect": ["__cuda", "__cuda_arch"],
                    "rocm-detect": ["__rocm"]
                }
            },
            "packages": {},
            "packages.conda": {}
        }"#;
        let repodata: RepoData = serde_json::from_str(raw).unwrap();
        let plugins = &repodata.info.as_ref().unwrap().virtual_package_plugins;

        assert_eq!(
            plugins
                .keys()
                .map(PackageName::as_source)
                .collect::<Vec<_>>(),
            ["cuda-detect", "rocm-detect"]
        );
        assert_eq!(
            plugins[&PackageName::new_unchecked("cuda-detect")]
                .iter()
                .map(PackageName::as_source)
                .collect::<Vec<_>>(),
            ["__cuda", "__cuda_arch"]
        );
        assert_eq!(
            plugins[&PackageName::new_unchecked("rocm-detect")]
                .iter()
                .map(PackageName::as_source)
                .collect::<Vec<_>>(),
            ["__rocm"]
        );

        let json = serde_json::to_string(&repodata).unwrap();
        assert!(json.contains("\"virtual_package_plugins\""));
        assert_eq!(serde_json::from_str::<RepoData>(&json).unwrap(), repodata);

        let without = RepoData {
            version: Some(2),
            info: Some(ChannelInfo {
                subdir: Some("linux-64".to_string()),
                base_url: None,
                repodata_revisions: IndexMap::default(),
                channel_relations: None,
                virtual_package_plugins: VirtualPackagePlugins::default(),
            }),
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: ahash::HashSet::default(),
        };
        assert!(
            !serde_json::to_string(&without)
                .unwrap()
                .contains("virtual_package_plugins")
        );
    }

    /// Names are deserialized unchecked, so a channel publishing a malformed
    /// name does not render the entire repodata unusable.
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[test]
    fn test_virtual_package_plugins_malformed_name() {
        let raw = r#"{
            "info": {
                "subdir": "linux-64",
                "virtual_package_plugins": { "invalid$plugin": ["__rocm"] }
            },
            "packages": {},
            "packages.conda": {}
        }"#;
        let repodata: RepoData = serde_json::from_str(raw).unwrap();
        assert_eq!(
            repodata
                .info
                .as_ref()
                .unwrap()
                .virtual_package_plugins
                .keys()
                .map(PackageName::as_source)
                .collect::<Vec<_>>(),
            ["invalid$plugin"]
        );
    }

    #[test]
    fn test_repodata_revisions() {
        let raw = r#"{
            "info": {
                "subdir": "linux-64",
                "repodata_revisions": {
                    "v4": {
                        "message": "new artifact types available",
                        "n_packages": 2,
                        "oldest": 1768249989851,
                        "newest": 1773851561010
                    }
                }
            },
            "packages": {},
            "packages.conda": {}
        }"#;

        let repodata: RepoData = serde_json::from_str(raw).unwrap();
        let revisions = &repodata.info.as_ref().unwrap().repodata_revisions;
        assert_eq!(revisions.len(), 1);
        let metadata = &revisions[&RepodataRevision::from(4)];
        assert_eq!(
            metadata.message.as_deref(),
            Some("new artifact types available")
        );
        assert_eq!(metadata.n_packages, Some(2));
        assert_eq!(
            metadata.oldest.map(|ts| ts.timestamp_millis()),
            Some(1768249989851)
        );
        assert_eq!(
            metadata.newest.map(|ts| ts.timestamp_millis()),
            Some(1773851561010)
        );

        let json = serde_json::to_string(&repodata).unwrap();
        assert!(json.contains("\"repodata_revisions\":{\"v4\":{"));
        assert!(json.contains("\"message\":\"new artifact types available\""));
        assert!(json.contains("\"oldest\":1768249989851"));
        assert!(json.contains("\"newest\":1773851561010"));
        // The revision identifier is the map key, not a field of the value.
        assert!(!json.contains("\"revision\""));

        let info = crate::RepodataRevisionInfo {
            revision: RepodataRevision::from(4),
            message: metadata.message.clone(),
            n_packages: metadata.n_packages,
            oldest: metadata.oldest,
            newest: metadata.newest,
        };
        assert_eq!(info.message, metadata.message);
        assert_eq!(info.n_packages, Some(2));
        assert_eq!(info.oldest, metadata.oldest);
        assert_eq!(info.newest, metadata.newest);
        let flattened = serde_json::to_value(&info).unwrap();
        assert_eq!(flattened["revision"], 4);
        assert_eq!(flattened["message"], "new artifact types available");
        assert_eq!(flattened["n_packages"], 2);
        assert_eq!(flattened["oldest"], 1768249989851i64);
        assert_eq!(flattened["newest"], 1773851561010i64);
        assert_eq!(
            serde_json::from_value::<crate::RepodataRevisionInfo>(flattened).unwrap(),
            info
        );
    }

    #[test]
    fn test_repodata_revisions_preserve_unknown_numeric_keys() {
        let raw = serde_json::json!({
            "info": {
                "subdir": null,
                "repodata_revisions": {
                    "v0": { "message": "legacy package maps" },
                    "v1": { "n_packages": 1 },
                    "v2": { "n_packages": 2 }
                }
            },
            "packages": {},
            "packages.conda": {},
            "repodata_version": 2
        });

        let repodata: RepoData = serde_json::from_value(raw.clone()).unwrap();
        let revisions = &repodata.info.as_ref().unwrap().repodata_revisions;
        assert_eq!(revisions.len(), 3);
        assert_eq!(
            revisions[&RepodataRevision::Legacy].message.as_deref(),
            Some("legacy package maps")
        );
        assert_eq!(revisions[&RepodataRevision::Unknown(1)].n_packages, Some(1));
        assert_eq!(revisions[&RepodataRevision::Unknown(2)].n_packages, Some(2));
        assert_eq!(serde_json::to_value(&repodata).unwrap(), raw);
    }

    #[test]
    fn test_repodata_readers_accept_future_producer_maps() {
        let repodata: RepoData = serde_json::from_value(serde_json::json!({
            "packages": {},
            "packages.conda": {},
            "v4": { "future": "data" },
            "repodata_version": 2
        }))
        .unwrap();
        assert!(repodata.v3.is_empty());
    }

    #[test]
    fn test_repodata_revision_keys_always_use_vn_format() {
        for (revision, key) in [
            (RepodataRevision::Legacy, "v0"),
            (RepodataRevision::Unknown(1), "v1"),
            (RepodataRevision::Unknown(2), "v2"),
            (RepodataRevision::V3, "v3"),
            (RepodataRevision::Unknown(4), "v4"),
        ] {
            assert_eq!(revision.to_string(), key);
        }

        // Continue accepting the former spelling when reading configuration,
        // but always write the CEP `vN` spelling.
        assert_eq!(
            "legacy".parse::<RepodataRevision>().unwrap(),
            RepodataRevision::Legacy
        );
    }

    #[test]
    fn test_repodata_revision_message_reader_is_permissive() {
        let message = "m".repeat(8193);
        let raw = serde_json::json!({ "message": message });
        let metadata: crate::RepodataRevisionMetadata =
            serde_json::from_value(raw.clone()).unwrap();

        assert_eq!(
            metadata.message.as_deref(),
            Some(raw["message"].as_str().unwrap())
        );
        assert_eq!(serde_json::to_value(metadata).unwrap(), raw);
    }

    #[test]
    fn test_v3_extensions_roundtrip() {
        let raw = serde_json::json!({
            "info": null,
            "packages": {},
            "packages.conda": {},
            "v3": {
                "conda": {
                    "demo-1.0-0": {
                        "build": "0",
                        "build_number": 0,
                        "name": "demo",
                        "subdir": "noarch",
                        "version": "1.0"
                    }
                },
                "zip": {
                    "demo-1.0-0": {
                        "future_field": ["preserve", true]
                    }
                }
            },
            "repodata_version": 1
        });

        let repodata: RepoData = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(repodata.v3.conda.len(), 1);
        assert_eq!(repodata.v3.extensions.get("zip"), Some(&raw["v3"]["zip"]));

        let serialized = serde_json::to_string(&repodata).unwrap();
        let reparsed: RepoData = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reparsed, repodata);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&serialized).unwrap()["v3"]["zip"],
            raw["v3"]["zip"]
        );
    }

    #[test]
    fn test_v3_extensions_permissively_roundtrip_all_json_bucket_shapes() {
        let raw = serde_json::json!({
            "info": null,
            "packages": {},
            "packages.conda": {},
            "v3": {
                "future-null": null,
                "future-scalar": "opaque",
                "future-array": ["opaque", { "nested-null": null }],
                "future-object": { "nested": { "preserve": true } }
            },
            "repodata_version": 1
        });

        let repodata: RepoData = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&repodata).unwrap(), raw);
        assert_eq!(
            serde_json::from_value::<RepoData>(serde_json::to_value(&repodata).unwrap()).unwrap(),
            repodata
        );
    }

    #[test]
    fn test_v3_extensions_reject_reserved_bucket_names() {
        let mut extensions = V3Extensions::default();
        for reserved in ["conda", "tar.bz2", "whl"] {
            let err = extensions
                .insert(reserved, serde_json::json!({}))
                .unwrap_err();
            assert_eq!(err.extension(), reserved);
        }
        extensions
            .insert("zip", serde_json::json!({"future": true}))
            .unwrap();

        let raw = serde_json::json!({
            "info": null,
            "packages": {},
            "packages.conda": {},
            "v3": {
                "tar.bz2": {
                    "demo-1.0-0": {
                        "build": "0",
                        "build_number": 0,
                        "name": "demo",
                        "subdir": "noarch",
                        "version": "1.0"
                    }
                },
                "conda": {
                    "demo-1.0-0": {
                        "build": "0",
                        "build_number": 0,
                        "name": "demo",
                        "subdir": "noarch",
                        "version": "1.0"
                    }
                },
                "whl": {
                    "demo-1.0-0": {
                        "build": "0",
                        "build_number": 0,
                        "name": "demo",
                        "subdir": "noarch",
                        "url": "demo-1.0-0.whl",
                        "version": "1.0"
                    }
                }
            },
            "repodata_version": 1
        });
        let mut repodata: RepoData = serde_json::from_value(raw).unwrap();
        repodata.v3.extensions = extensions;

        let serialized = serde_json::to_string(&repodata).unwrap();
        for reserved in ["conda", "tar.bz2", "whl"] {
            assert_eq!(
                serialized.matches(&format!(r#""{reserved}":{{"#)).count(),
                1,
                "{reserved} must be emitted only as its typed v3 bucket"
            );
        }
        let reparsed: RepoData = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reparsed.v3.tar_bz2.len(), 1);
        assert_eq!(reparsed.v3.conda.len(), 1);
        assert_eq!(reparsed.v3.whl.len(), 1);
        assert_eq!(
            reparsed.v3.extensions.get("zip"),
            Some(&serde_json::json!({"future": true}))
        );
    }

    #[test]
    fn test_package_record_timestamp_for_indexing() {
        let timestamp = crate::utils::TimestampMs::from_timestamp_millis(
            jiff::Timestamp::from_millisecond(1_700_000_000_000).unwrap(),
        );
        let mut record = PackageRecord::new(
            crate::PackageName::new_unchecked("demo"),
            crate::Version::major(1),
            "0".to_string(),
        );

        assert_eq!(record.timestamp_for_indexing(), None);
        record.timestamp = Some(timestamp);
        assert_eq!(record.timestamp_for_indexing(), Some(timestamp));
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
            v3: V3Packages::default(),
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

    #[test]
    fn test_ordering() {
        use crate::{PackageName, Version};

        let record = |name: &str,
                      version: &str,
                      build: &str,
                      build_number: u64,
                      subdir: &str,
                      timestamp: Option<i64>|
         -> PackageRecord {
            let mut r = PackageRecord::new(
                PackageName::new_unchecked(name),
                version.parse::<Version>().unwrap(),
                format!("{build}_{build_number}"),
            );
            r.build_number = build_number;
            r.subdir = subdir.to_string();
            r.timestamp = timestamp.map(|secs| {
                crate::utils::TimestampMs::from_timestamp_seconds(
                    jiff::Timestamp::from_second(secs).unwrap(),
                )
            });
            r
        };

        let mut records = vec![
            // Different versions of the same package
            record("python", "3.12.0", "hab5_py312", 3, "linux-64", None),
            record("python", "3.11.0", "hab5_py311", 1, "linux-64", None),
            record("python", "3.12.0", "hab5_py312", 1, "linux-64", None),
            // Different build numbers
            record("numpy", "1.26.0", "hc1_np126", 2, "linux-64", None),
            record("numpy", "1.26.0", "hc1_np126", 0, "linux-64", None),
            record("numpy", "1.26.0", "hc1_np126", 1, "linux-64", None),
            // Different timestamps (same version & build number)
            record("openssl", "3.1.0", "hlib", 0, "linux-64", Some(1700000000)),
            record("openssl", "3.1.0", "hlib", 0, "linux-64", Some(1600000000)),
            record("openssl", "3.1.0", "hlib", 0, "linux-64", Some(1800000000)),
            // Track features (packages with tracked features sort before those
            // without)
            {
                let mut r = record("scipy", "1.11.0", "hfeature", 0, "linux-64", None);
                r.track_features = vec!["mkl".to_string()];
                r
            },
            record("scipy", "1.11.0", "hplain", 0, "linux-64", None),
            // Another package to show name ordering
            record("curl", "8.4.0", "hdns", 0, "linux-64", None),
        ];

        records.sort();

        let formatted: Vec<String> = records
            .iter()
            .map(|r| {
                format!(
                    "{}/{}-{}-{}",
                    r.subdir,
                    r.name.as_normalized(),
                    r.version,
                    r.build
                )
            })
            .collect();
        insta::assert_snapshot!(formatted.join("\n"));
    }

    #[test]
    fn test_ordering_track_features_vs_version() {
        use crate::{PackageName, Version};

        let record =
            |version: &str, build: &str, build_number: u64, track_features: Vec<String>| {
                let mut r = PackageRecord::new(
                    PackageName::new_unchecked("polars"),
                    version.parse::<Version>().unwrap(),
                    format!("{build}_{build_number}"),
                );
                r.build_number = build_number;
                r.subdir = "linux-64".to_string();
                r.track_features = track_features;
                r
            };

        let with_track = record("1.33.0", "withtrack", 0, vec!["u64_idx".to_string()]);
        let no_track_old = record("0.28.0", "plain", 0, vec![]);
        let no_track_same = record("1.33.0", "plain", 0, vec![]);
        let no_track_new = record("1.38.0", "plain", 0, vec![]);

        assert!(with_track < no_track_old);
        assert!(no_track_old < no_track_same);
        assert!(no_track_same < no_track_new);
    }
}
