use rattler_conda_types::package::DistArchiveIdentifier;
use std::{borrow::Cow, cmp::Ordering, collections::BTreeMap, fmt::Display, hash::Hash};

use rattler_conda_types::{
    ChannelUrl, MatchSpec, Matches, NamelessMatchSpec, PackageRecord, RepoDataRecord,
};
use rattler_digest::Sha256Hash;
use serde::{Deserialize, Serialize};
use typed_path::Utf8TypedPathBuf;
use url::Url;

use crate::{source::SourceLocation, UrlOrPath};

/// Represents a conda-build variant value.
///
/// Variants are used in conda-build to specify different build configurations.
/// They can be strings (e.g., "3.11" for python version), integers (e.g., 1 for
/// feature flags), or booleans (e.g., true/false for optional features).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VariantValue {
    /// String variant value (most common, e.g., python version "3.11")
    String(String),
    /// Integer variant value (e.g., for numeric feature flags)
    Int(i64),
    /// Boolean variant value (e.g., for on/off features)
    Bool(bool),
}

impl PartialOrd for VariantValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VariantValue {
    fn cmp(&self, other: &Self) -> Ordering {
        #[allow(clippy::match_same_arms)]
        match (self, other) {
            (VariantValue::String(a), VariantValue::String(b)) => a.cmp(b),
            (VariantValue::Int(a), VariantValue::Int(b)) => a.cmp(b),
            (VariantValue::Bool(a), VariantValue::Bool(b)) => a.cmp(b),
            // Define ordering between different types for deterministic sorting
            (VariantValue::String(_), _) => Ordering::Less,
            (_, VariantValue::String(_)) => Ordering::Greater,
            (VariantValue::Int(_), VariantValue::Bool(_)) => Ordering::Less,
            (VariantValue::Bool(_), VariantValue::Int(_)) => Ordering::Greater,
        }
    }
}

impl Display for VariantValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VariantValue::String(s) => write!(f, "{s}"),
            VariantValue::Int(i) => write!(f, "{i}"),
            VariantValue::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// A locked conda dependency can be either a binary package or a source
/// package.
///
/// A binary package is a package that is already built and can be installed
/// directly.
///
/// A source package is a package that needs to be built before it can
/// be installed. Although the source package is not built, it does contain
/// dependency information through the [`PackageRecord`] struct.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CondaPackageData {
    /// A binary package. A binary package is identified by looking at the
    /// location or filename of the package file and seeing if it represents a
    /// valid binary package name.
    Binary(CondaBinaryData),

    /// A source package.
    Source(CondaSourceData),
}

impl CondaPackageData {
    /// Returns the location of the package.
    pub fn location(&self) -> &UrlOrPath {
        match self {
            Self::Binary(data) => &data.location,
            Self::Source(data) => &data.location,
        }
    }

    /// Returns the dependency information of the package.
    ///
    /// For binary packages, this always returns `Some`. For source packages,
    /// the record may not be present if the metadata hasn't been evaluated yet.
    pub fn record(&self) -> Option<&PackageRecord> {
        match self {
            CondaPackageData::Binary(data) => Some(&data.package_record),
            CondaPackageData::Source(data) => data.package_record.as_ref(),
        }
    }

    /// Returns the name of the package.
    pub fn name(&self) -> &rattler_conda_types::PackageName {
        match self {
            CondaPackageData::Binary(data) => &data.package_record.name,
            CondaPackageData::Source(data) => &data.name,
        }
    }

    /// Returns a reference to the binary representation of this instance if it
    /// exists.
    pub fn as_binary(&self) -> Option<&CondaBinaryData> {
        match self {
            Self::Binary(data) => Some(data),
            Self::Source(_) => None,
        }
    }

    /// Returns a reference to the source representation of this instance if it
    /// exists.
    pub fn as_source(&self) -> Option<&CondaSourceData> {
        match self {
            Self::Binary(_) => None,
            Self::Source(data) => Some(data),
        }
    }

    /// Returns the binary representation of this instance if it exists.
    pub fn into_binary(self) -> Option<CondaBinaryData> {
        match self {
            Self::Binary(data) => Some(data),
            Self::Source(_) => None,
        }
    }

    /// Returns the source representation of this instance if it exists.
    pub fn into_source(self) -> Option<CondaSourceData> {
        match self {
            Self::Binary(_) => None,
            Self::Source(data) => Some(data),
        }
    }
}

/// Information about a binary conda package stored in the lock-file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CondaBinaryData {
    /// The package record.
    pub package_record: PackageRecord,

    /// The location of the package. This can be a URL or a local path.
    pub location: UrlOrPath,

    /// The filename of the package.
    pub file_name: DistArchiveIdentifier,

    /// The channel of the package.
    pub channel: Option<ChannelUrl>,
}

impl From<CondaBinaryData> for CondaPackageData {
    fn from(value: CondaBinaryData) -> Self {
        Self::Binary(value)
    }
}

impl CondaBinaryData {
    pub(crate) fn merge(&self, other: &Self) -> Cow<'_, Self> {
        if self.location == other.location {
            if let Cow::Owned(merged) =
                merge_package_record(&self.package_record, &other.package_record)
            {
                return Cow::Owned(Self {
                    package_record: merged,
                    ..self.clone()
                });
            }
        }

        Cow::Borrowed(self)
    }
}

/// Shallow git specification for tracking the originally requested reference.
///
/// This allows detecting when the source specification has changed (e.g., from
/// branch `main` to branch `dev`) even if the current commit hash is the same.
/// Without this information, we wouldn't know if the lock file needs to be
/// updated when the requested branch/tag changes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitShallowSpec {
    /// A git branch reference (e.g., "main", "develop")
    Branch(String),
    /// A git tag reference (e.g., "v1.0.0", "release-2023")
    Tag(String),
    /// Revision here means that original manifest explicitly pinned revision.
    Rev,
}

/// Package build source location for reproducible builds.
///
/// This stores the exact source location information needed to
/// reproducibly build a package from source. Used by pixi build
/// and other package building tools.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PackageBuildSource {
    /// Git repository source with specific revision.
    Git {
        /// The repository URL.
        url: Url,
        /// Shallow specification of repository head to use.
        ///
        /// Needed to detect if we have to recompute revision.
        spec: Option<GitShallowSpec>,
        /// The specific git revision.
        rev: String,
        /// Subdirectory on which focus on.
        subdir: Option<Utf8TypedPathBuf>,
    },
    /// URL-based archive source with content hash.
    Url {
        /// The URL to the archive.
        url: Url,
        /// The SHA256 hash of the archive content.
        sha256: Sha256Hash,
        /// Subdirectory to use.
        subdir: Option<Utf8TypedPathBuf>,
    },
    /// Source is some local path.
    Path {
        /// Actual path.
        path: Utf8TypedPathBuf,
    },
}

/// Information about a source package stored in the lock-file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CondaSourceData {
    /// The location of the package. This can be a URL or a local path.
    pub location: UrlOrPath,

    /// Package build source location for reproducible builds
    pub package_build_source: Option<PackageBuildSource>,

    /// The name of the output to use from the source.
    pub name: rattler_conda_types::PackageName,

    /// Conda-build variants used to disambiguate between multiple source
    /// packages at the same location. This is a map from variant name to
    /// variant value. Optional field added in lock file format V6 (made
    /// required in V7).
    pub variants: BTreeMap<String, VariantValue>,

    /// Optionally, the evaluated package record.
    ///
    /// We either force all metadata to be present for a source package or None.
    /// If the data is not present, it should be retrieved at runtime.
    pub package_record: Option<PackageRecord>,

    /// Information about packages that should be built from source instead of
    /// binary. This maps from a normalized package name to the location of the
    /// source.
    pub sources: BTreeMap<String, SourceLocation>,
}

impl From<CondaSourceData> for CondaPackageData {
    fn from(value: CondaSourceData) -> Self {
        Self::Source(value)
    }
}

/// A record of input files that were used to define the metadata of the
/// package.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InputHash {
    /// The hash of all input files combined.
    pub hash: Sha256Hash,

    /// The globs that were used to define the input files.
    pub globs: Vec<String>,
}

impl PartialOrd<Self> for CondaPackageData {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CondaPackageData {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare by location first
        self.location()
            .cmp(other.location())
            // Then by name
            .then_with(|| self.name().cmp(other.name()))
            // Then by additional fields from package records if available
            .then_with(|| match (self.record(), other.record()) {
                (Some(pkg_a), Some(pkg_b)) => pkg_a
                    .version
                    .cmp(&pkg_b.version)
                    .then_with(|| pkg_a.build.cmp(&pkg_b.build))
                    .then_with(|| pkg_a.subdir.cmp(&pkg_b.subdir)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            })
            // For source packages, also compare by variants and build source as tiebreakers
            .then_with(|| match (self.as_source(), other.as_source()) {
                (Some(src_a), Some(src_b)) => src_a
                    .variants
                    .cmp(&src_b.variants)
                    .then_with(|| src_a.package_build_source.cmp(&src_b.package_build_source)),
                _ => Ordering::Equal,
            })
    }
}

impl From<RepoDataRecord> for CondaPackageData {
    fn from(value: RepoDataRecord) -> Self {
        let location = UrlOrPath::from(value.url).normalize().into_owned();
        Self::Binary(CondaBinaryData {
            package_record: value.package_record,
            file_name: value.identifier,
            channel: value
                .channel
                .and_then(|channel| Url::parse(&channel).ok())
                .map(Into::into),
            location,
        })
    }
}

impl TryFrom<&CondaBinaryData> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: &CondaBinaryData) -> Result<Self, Self::Error> {
        Self::try_from(value.clone())
    }
}

impl TryFrom<CondaBinaryData> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: CondaBinaryData) -> Result<Self, Self::Error> {
        Ok(Self {
            package_record: value.package_record,
            identifier: value.file_name,
            url: value.location.try_into_url()?,
            channel: value.channel.map(|channel| channel.to_string()),
        })
    }
}

/// Error used when converting from `repo_data` module to conda lock module
#[derive(thiserror::Error, Debug)]
pub enum ConversionError {
    /// This field was found missing during the conversion
    #[error("missing field/fields '{0}'")]
    Missing(String),

    /// The location of the conda package cannot be converted to a URL
    #[error(transparent)]
    LocationToUrlConversionError(#[from] file_url::FileURLParseError),

    /// The location does not have a valid binary package filename (e.g.,
    /// `.conda` or `.tar.bz2`)
    #[error("binary package location must have a valid archive filename (.conda or .tar.bz2)")]
    InvalidBinaryPackageLocation,
}

impl CondaPackageData {
    /// Returns true if this package satisfies the given `spec`.
    pub fn satisfies(&self, spec: &MatchSpec) -> bool {
        self.matches(spec)
    }
}

impl Matches<MatchSpec> for CondaPackageData {
    fn matches(&self, spec: &MatchSpec) -> bool {
        // Check if the name matches
        if let Some(name) = &spec.name {
            if !name.matches(self.name()) {
                return false;
            }
        }

        // Check if the channel matches
        if let Some(channel) = &spec.channel {
            match self {
                CondaPackageData::Binary(binary) => {
                    if let Some(record_channel) = &binary.channel {
                        if &channel.base_url != record_channel {
                            return false;
                        }
                    }
                }
                CondaPackageData::Source(_) => {
                    return false;
                }
            }
        }

        // Check if the record matches (if available)
        match self.record() {
            Some(record) => spec.matches(record),
            None => {
                // Source packages without a record can only match if the spec
                // only constrains the name (which we already checked above)
                spec.version.is_none()
                    && spec.build.is_none()
                    && spec.build_number.is_none()
                    && spec.subdir.is_none()
                    && spec.md5.is_none()
                    && spec.sha256.is_none()
            }
        }
    }
}

impl Matches<NamelessMatchSpec> for CondaPackageData {
    fn matches(&self, spec: &NamelessMatchSpec) -> bool {
        // Check if the channel matches
        if let Some(channel) = &spec.channel {
            match self {
                CondaPackageData::Binary(binary) => {
                    if let Some(record_channel) = &binary.channel {
                        if &channel.base_url != record_channel {
                            return false;
                        }
                    }
                }
                CondaPackageData::Source(_) => {
                    return false;
                }
            }
        }

        // Check if the record matches (if available)
        match self.record() {
            Some(record) => spec.matches(record),
            None => {
                // Source packages without a record can only match if the spec
                // has no constraints
                spec.version.is_none()
                    && spec.build.is_none()
                    && spec.build_number.is_none()
                    && spec.subdir.is_none()
                    && spec.md5.is_none()
                    && spec.sha256.is_none()
            }
        }
    }
}

fn merge_package_record<'a>(
    left: &'a PackageRecord,
    right: &PackageRecord,
) -> Cow<'a, PackageRecord> {
    let mut result = Cow::Borrowed(left);

    // If the left package doesn't contain purls we merge those from the right one.
    if left.purls.is_none() && right.purls.is_some() {
        result = Cow::Owned(PackageRecord {
            purls: right.purls.clone(),
            ..result.into_owned()
        });
    }

    // If the left package doesn't contain run_exports we merge those from the right
    // one.
    if left.run_exports.is_none() && right.run_exports.is_some() {
        result = Cow::Owned(PackageRecord {
            run_exports: right.run_exports.clone(),
            ..result.into_owned()
        });
    }

    // Merge hashes if the left package doesn't contain them.
    if left.md5.is_none() && right.md5.is_some() {
        result = Cow::Owned(PackageRecord {
            md5: right.md5,
            ..result.into_owned()
        });
    }
    if left.sha256.is_none() && right.sha256.is_some() {
        result = Cow::Owned(PackageRecord {
            sha256: right.sha256,
            ..result.into_owned()
        });
    }

    result
}
