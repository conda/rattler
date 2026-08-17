use rattler_conda_types::package::DistArchiveIdentifier;
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    hash::Hash,
};

use rattler_conda_types::{
    ChannelUrl, Flag, MatchSpec, Matches, NamelessMatchSpec, PackageRecord, PackageUrl,
    RepoDataRecord, package::RunExportsJson,
};
use rattler_digest::Sha256Hash;
use serde::{Deserialize, Serialize};
use typed_path::Utf8TypedPathBuf;
use url::Url;

use crate::{SourceData, UrlOrPath, Verbatim, source::SourceLocation};

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
    Binary(Box<CondaBinaryData>),

    /// A source package.
    Source(Box<CondaSourceData>),
}

impl CondaPackageData {
    /// Returns the location of the package.
    pub fn location(&self) -> &UrlOrPath {
        match self {
            Self::Binary(data) => data.location.inner(),
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
            CondaPackageData::Source(data) => data.record(),
        }
    }

    /// Returns the name of the package.
    pub fn name(&self) -> &rattler_conda_types::PackageName {
        match self {
            CondaPackageData::Binary(data) => &data.package_record.name,
            CondaPackageData::Source(data) => data.name(),
        }
    }

    /// Returns the dependencies of the package.
    ///
    /// For binary packages, this returns the dependencies from the package
    /// record. For source packages, this returns the dependencies from either
    /// the full record or the partial metadata.
    pub fn depends(&self) -> &[String] {
        match self {
            CondaPackageData::Binary(data) => &data.package_record.depends,
            CondaPackageData::Source(data) => data.depends(),
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
            Self::Binary(data) => Some(*data),
            Self::Source(_) => None,
        }
    }

    /// Returns the source representation of this instance if it exists.
    pub fn into_source(self) -> Option<CondaSourceData> {
        match self {
            Self::Binary(_) => None,
            Self::Source(data) => Some(*data),
        }
    }
}

/// Information about a binary conda package stored in the lock-file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CondaBinaryData {
    /// The package record.
    pub package_record: PackageRecord,

    /// The location of the package. This can be a URL or a local path.
    ///
    /// Stored verbatim so a relative path round-trips through the lock file:
    /// the `given` string is what was written on disk, while the inner value
    /// is left relative for the consumer to resolve against the lock file's
    /// base directory.
    pub location: Verbatim<UrlOrPath>,

    /// The filename of the package.
    pub file_name: DistArchiveIdentifier,

    /// The channel of the package.
    pub channel: Option<ChannelUrl>,
}

impl From<CondaBinaryData> for CondaPackageData {
    fn from(value: CondaBinaryData) -> Self {
        Self::Binary(Box::new(value))
    }
}

impl CondaBinaryData {
    pub(crate) fn merge(&self, other: &Self) -> Cow<'_, Self> {
        if self.location == other.location
            && let Cow::Owned(merged) =
                merge_package_record(&self.package_record, &other.package_record)
        {
            return Cow::Owned(Self {
                package_record: merged,
                ..self.clone()
            });
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

/// Metadata for a source package without a fully evaluated package record.
///
/// Contains the package name plus any known dependency and source information,
/// but lacks the full [`PackageRecord`] fields (version, build, subdir, etc.).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PartialSourceMetadata {
    /// The name of the output to use from the source.
    pub name: rattler_conda_types::PackageName,

    /// Dependencies on other packages (run-time requirements).
    pub depends: Vec<String>,

    /// Run-constraints on other packages.
    pub constrains: Vec<String>,

    /// Additional dependencies grouped by an extra/feature key.
    pub extra_depends: BTreeMap<String, Vec<String>>,

    /// Variant-selection flags declared by the recipe.
    pub flags: Vec<Flag>,

    /// License of the package (SPDX expression or free-form string).
    pub license: Option<String>,

    /// PURLs (Package URLs) describing this package in other ecosystems.
    pub purls: Option<BTreeSet<PackageUrl>>,

    /// Run-exports declared by the recipe.
    pub run_exports: Option<RunExportsJson>,
}

/// Metadata for a source package, either partial (name-only) or full
/// (evaluated record + sources).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceMetadata {
    /// Only the package name is known.
    Partial(Box<PartialSourceMetadata>),
    /// The package has been fully evaluated.
    Full(Box<PackageRecord>),
}

impl SourceMetadata {
    /// Returns a reference to the full metadata if this is the `Full` variant.
    pub fn as_full(&self) -> Option<&PackageRecord> {
        match self {
            Self::Full(full) => Some(full),
            Self::Partial(_) => None,
        }
    }

    /// Returns a reference to the partial metadata if this is the `Partial`
    /// variant.
    pub fn as_partial(&self) -> Option<&PartialSourceMetadata> {
        match self {
            Self::Partial(partial) => Some(partial),
            Self::Full(_) => None,
        }
    }
}

/// Information about a source package stored in the lock-file.
///
/// The type parameter `D` determines the metadata level:
/// - [`SourceMetadata`] (default): either partial or full metadata
/// - [`PackageRecord`]: a full package record
/// - [`PartialSourceMetadata`]: only the name is known
#[derive(Clone, Debug)]
pub struct CondaSourceData<D = SourceMetadata> {
    /// The location of the package. This can be a URL or a local path.
    pub location: UrlOrPath,

    /// Package build source location for reproducible builds
    pub package_build_source: Option<PackageBuildSource>,

    /// Conda-build variants used to disambiguate between multiple source
    /// packages at the same location. This is a map from variant name to
    /// variant value. Optional field added in lock file format V6 (made
    /// required in V7).
    pub variants: BTreeMap<String, VariantValue>,

    /// The short hash that was originally parsed from the [`crate::SourceIdentifier`]
    /// in the lock file (e.g. the `9f3c2a7b` part of `numba-cuda[9f3c2a7b] @ .`).
    ///
    /// When `Some`, [`crate::SourceIdentifier::from_source_data`] reuses this
    /// value verbatim rather than recomputing the hash. This ensures that
    /// round-tripping a lock file produces no spurious changes to source
    /// identifiers even if the hash algorithm or its inputs evolve over time.
    ///
    /// This field is intentionally excluded from [`PartialEq`], [`Eq`], and
    /// [`Hash`] because it carries no semantic meaning about the package itself.
    pub identifier_hash: Option<String>,

    /// Information about packages that should be built from source instead of
    /// binary. This maps from a normalized package name to the location of the
    /// source.
    pub sources: BTreeMap<String, SourceLocation>,

    /// The build and host environment packages needed to build this source
    /// package.
    pub source_data: SourceData,

    /// The metadata for this source package.
    pub metadata: D,
}

impl<D: PartialEq> PartialEq for CondaSourceData<D> {
    fn eq(&self, other: &Self) -> bool {
        self.location == other.location
            && self.package_build_source == other.package_build_source
            && self.variants == other.variants
            && self.metadata == other.metadata
    }
}

impl<D: Eq> Eq for CondaSourceData<D> {}

impl<D: Hash> std::hash::Hash for CondaSourceData<D> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.location.hash(state);
        self.package_build_source.hash(state);
        self.variants.hash(state);
        self.metadata.hash(state);
    }
}

// --- Methods for CondaSourceData<SourceMetadata> (default) ---

impl CondaSourceData<SourceMetadata> {
    /// Returns the name of the package.
    pub fn name(&self) -> &rattler_conda_types::PackageName {
        match &self.metadata {
            SourceMetadata::Partial(p) => &p.name,
            SourceMetadata::Full(f) => &f.name,
        }
    }

    /// Returns the package record if the metadata has been fully evaluated.
    pub fn record(&self) -> Option<&PackageRecord> {
        self.metadata.as_full()
    }

    /// Returns the source locations.
    pub fn sources(&self) -> &BTreeMap<String, SourceLocation> {
        &self.sources
    }

    /// Returns the dependencies. Empty for partial metadata without depends.
    pub fn depends(&self) -> &[String] {
        match &self.metadata {
            SourceMetadata::Full(f) => &f.depends,
            SourceMetadata::Partial(p) => &p.depends,
        }
    }

    /// Returns the run-constraints. Empty for partial metadata without
    /// constrains.
    pub fn constrains(&self) -> &[String] {
        match &self.metadata {
            SourceMetadata::Full(f) => &f.constrains,
            SourceMetadata::Partial(p) => &p.constrains,
        }
    }

    /// Attempts to convert into a `CondaSourceData<PackageRecord>`.
    /// Returns `None` if the metadata is partial.
    pub fn into_full(self) -> Option<CondaSourceData<PackageRecord>> {
        match self.metadata {
            SourceMetadata::Full(full) => Some(CondaSourceData {
                location: self.location,
                package_build_source: self.package_build_source,
                variants: self.variants,
                identifier_hash: self.identifier_hash,
                sources: self.sources,
                source_data: self.source_data,
                metadata: *full,
            }),
            SourceMetadata::Partial(_) => None,
        }
    }

    /// Convenience constructor for a full source data.
    pub fn full(
        location: UrlOrPath,
        package_build_source: Option<PackageBuildSource>,
        variants: BTreeMap<String, VariantValue>,
        identifier_hash: Option<String>,
        package_record: PackageRecord,
        sources: BTreeMap<String, SourceLocation>,
    ) -> Self {
        Self {
            location,
            package_build_source,
            variants,
            identifier_hash,
            sources,
            source_data: SourceData::default(),
            metadata: SourceMetadata::Full(Box::new(package_record)),
        }
    }

    /// Convenience constructor for a partial source data.
    #[allow(clippy::too_many_arguments)]
    pub fn partial(
        location: UrlOrPath,
        package_build_source: Option<PackageBuildSource>,
        variants: BTreeMap<String, VariantValue>,
        identifier_hash: Option<String>,
        name: rattler_conda_types::PackageName,
        depends: Vec<String>,
        constrains: Vec<String>,
        extra_depends: BTreeMap<String, Vec<String>>,
        flags: Vec<Flag>,
        license: Option<String>,
        purls: Option<BTreeSet<PackageUrl>>,
        run_exports: Option<RunExportsJson>,
        sources: BTreeMap<String, SourceLocation>,
    ) -> Self {
        Self {
            location,
            package_build_source,
            variants,
            identifier_hash,
            sources,
            source_data: SourceData::default(),
            metadata: SourceMetadata::Partial(Box::new(PartialSourceMetadata {
                name,
                depends,
                constrains,
                extra_depends,
                flags,
                license,
                purls,
                run_exports,
            })),
        }
    }
}

// --- Methods for CondaSourceData<PackageRecord> ---

impl CondaSourceData<PackageRecord> {
    /// Returns the name of the package.
    pub fn name(&self) -> &rattler_conda_types::PackageName {
        &self.metadata.name
    }

    /// Returns the package record.
    pub fn record(&self) -> &PackageRecord {
        &self.metadata
    }

    /// Returns the dependencies.
    pub fn depends(&self) -> &[String] {
        &self.metadata.depends
    }

    /// Returns the run-constraints.
    pub fn constrains(&self) -> &[String] {
        &self.metadata.constrains
    }
}

// --- Methods for CondaSourceData<PartialSourceMetadata> ---

impl CondaSourceData<PartialSourceMetadata> {
    /// Returns the name of the package.
    pub fn name(&self) -> &rattler_conda_types::PackageName {
        &self.metadata.name
    }

    /// Returns the dependencies.
    pub fn depends(&self) -> &[String] {
        &self.metadata.depends
    }

    /// Returns the run-constraints.
    pub fn constrains(&self) -> &[String] {
        &self.metadata.constrains
    }
}

// --- Conversions ---

impl From<CondaSourceData<PackageRecord>> for CondaSourceData<SourceMetadata> {
    fn from(value: CondaSourceData<PackageRecord>) -> Self {
        Self {
            location: value.location,
            package_build_source: value.package_build_source,
            variants: value.variants,
            identifier_hash: value.identifier_hash,
            sources: value.sources,
            source_data: value.source_data,
            metadata: SourceMetadata::Full(Box::new(value.metadata)),
        }
    }
}

impl From<CondaSourceData<PartialSourceMetadata>> for CondaSourceData<SourceMetadata> {
    fn from(value: CondaSourceData<PartialSourceMetadata>) -> Self {
        Self {
            location: value.location,
            package_build_source: value.package_build_source,
            variants: value.variants,
            identifier_hash: value.identifier_hash,
            sources: value.sources,
            source_data: value.source_data,
            metadata: SourceMetadata::Partial(Box::new(value.metadata)),
        }
    }
}

impl From<CondaSourceData> for CondaPackageData {
    fn from(value: CondaSourceData) -> Self {
        Self::Source(Box::new(value))
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

impl From<RepoDataRecord> for CondaPackageData {
    fn from(value: RepoDataRecord) -> Self {
        let location = UrlOrPath::from(value.url).normalize().into_owned();
        Self::Binary(Box::new(CondaBinaryData {
            package_record: value.package_record,
            file_name: value.identifier,
            channel: value
                .channel
                .and_then(|channel| Url::parse(&channel).ok())
                .map(Into::into),
            location: Verbatim::new(location),
        }))
    }
}

impl From<Url> for Verbatim<UrlOrPath> {
    fn from(url: Url) -> Self {
        Verbatim::new(UrlOrPath::from(url))
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
        if !spec.name.matches(self.name()) {
            return false;
        }

        // Check if the channel matches
        if let Some(channel) = &spec.channel {
            match self {
                CondaPackageData::Binary(binary) => {
                    if let Some(record_channel) = &binary.channel
                        && &channel.base_url != record_channel
                    {
                        return false;
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
                    if let Some(record_channel) = &binary.channel
                        && &channel.base_url != record_channel
                    {
                        return false;
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
    } else if let (Some(l), Some(r)) = (&left.run_exports, &right.run_exports)
        && l != r
    {
        tracing::debug!(
            package = %left.name.as_normalized(),
            "merging two records with conflicting run_exports; keeping the existing one"
        );
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

    if left.flags.is_empty() && !right.flags.is_empty() {
        result = Cow::Owned(PackageRecord {
            flags: right.flags.clone(),
            ..result.into_owned()
        });
    }

    result
}
