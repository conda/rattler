//! Query language for conda packages.
use crate::match_spec::condition::MatchSpecCondition;
use crate::package::CondaArchiveIdentifier;
use crate::{
    GenericVirtualPackage, PackageName, PackageRecord, RepoDataRecord, RepodataRevision,
    VersionSpec, build_spec::BuildNumberSpec,
};
use rattler_digest::{Md5, Sha256, parse_digest_from_hex};
use rattler_digest::{Md5Hash, Sha256Hash, serde::SerializableHash};
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::{serde_as, skip_serializing_none};
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use std::sync::Arc;
use url::Url;

use crate::Channel;
use crate::ChannelConfig;

/// Experimental conditionals for match specs.
pub mod condition;
/// The single renderer behind every textual representation of a match spec.
pub(crate) mod format;
/// Match a given string either by exact match, glob or regex
pub mod matcher;
/// Match package names either by exact match, glob or regex
pub mod package_name_matcher;
/// Parse a match spec from a string
pub mod parse;

use format::{DisplayContext, FormatError, SpecView};
use matcher::StringMatcher;
use package_name_matcher::PackageNameMatcher;

/// A [`MatchSpec`] is, fundamentally, a query language for conda packages. Any of the fields that
/// comprise a [`crate::PackageRecord`] can be used to compose a [`MatchSpec`].
///
/// [`MatchSpec`] can be composed with keyword arguments, where keys are any of the
/// attributes of [`crate::PackageRecord`]. Values for keyword arguments are the exact
/// values the attribute should match against. Many fields can also be matched against non-exact
/// values -- by including wildcard `*` and `>`/`<` ranges--where supported. Any non-specified field
/// is the equivalent of a full wildcard match.
///
/// `MatchSpecs` can also be composed using a single positional argument, with optional
/// keyword arguments. Keyword arguments also override any conflicting information provided in
/// the positional argument. Conda has historically had several string representations for equivalent
/// `MatchSpecs`.
///
/// [`Display`] preserves a historic, positional representation for backwards
/// compatibility. It is not a stable serialization format. Use
/// [`MatchSpec::to_canonical_string`] for deterministic, unambiguous output:
/// the package name is first, every populated non-name field is represented
/// in a single bracket section, and the same spec always renders to the same
/// string.
///
/// When `MatchSpec` attribute values are simple strings, the are interpreted using the
/// following conventions:
///   - If the string begins with `^` and ends with `$`, it is converted to a regex.
///   - If the string contains an asterisk (`*`), it is transformed from a glob to a regex.
///   - Otherwise, an exact match to the string is sought.
///
/// # Examples:
///
/// ```rust
/// use rattler_conda_types::{MatchSpec, VersionSpec, StringMatcher, PackageNameMatcher, PackageName, Channel, ChannelConfig, ParseStrictness::*};
/// use std::str::FromStr;
/// use std::sync::Arc;
///
/// let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
/// let spec = MatchSpec::from_str("foo 1.0.* py27_0", Strict).unwrap();
/// assert_eq!(spec.name, PackageNameMatcher::Exact(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str("1.0.*", Strict).unwrap()));
/// assert_eq!(spec.build, Some(StringMatcher::from_str("py27_0").unwrap()));
///
/// let spec = MatchSpec::from_str("foo ==1.0 py27_0", Strict).unwrap();
/// assert_eq!(spec.name, PackageNameMatcher::Exact(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str("==1.0", Strict).unwrap()));
/// assert_eq!(spec.build, Some(StringMatcher::from_str("py27_0").unwrap()));
///
/// let spec = MatchSpec::from_str(r#"conda-forge::foo[version="1.0.*"]"#, Strict).unwrap();
/// assert_eq!(spec.name, PackageNameMatcher::Exact(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str("1.0.*", Strict).unwrap()));
/// assert_eq!(spec.channel, Some(Channel::from_str("conda-forge", &channel_config).map(|channel| Arc::new(channel)).unwrap()));
///
/// let spec = MatchSpec::from_str(r#"conda-forge::foo >=1.0[subdir="linux-64"]"#, Strict).unwrap();
/// assert_eq!(spec.name, PackageNameMatcher::Exact(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str(">=1.0", Strict).unwrap()));
/// assert_eq!(spec.channel, Some(Channel::from_str("conda-forge", &channel_config).map(|channel| Arc::new(channel)).unwrap()));
/// assert_eq!(spec.subdir, Some("linux-64".to_string()));
/// assert_eq!(spec, MatchSpec::from_str("conda-forge/linux-64::foo >=1.0", Strict).unwrap());
///
/// let spec = MatchSpec::from_str("*/linux-64::foo >=1.0", Strict).unwrap();
/// assert_eq!(spec.name, PackageNameMatcher::Exact(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str(">=1.0", Strict).unwrap()));
/// assert_eq!(spec.channel, Some(Channel::from_str("*", &channel_config).map(|channel| Arc::new(channel)).unwrap()));
/// assert_eq!(spec.subdir, Some("linux-64".to_string()));
///
/// let spec = MatchSpec::from_str(r#"foo[build="py2*"]"#, Strict).unwrap();
/// assert_eq!(spec.name, PackageNameMatcher::Exact(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.build, Some(StringMatcher::from_str("py2*").unwrap()));
/// ```
///
/// To fully-specify a package with a full, exact spec, the following fields must be given as exact values:
///
///   - channel
///   - subdir
///   - name
///   - version
///   - build
///
/// In the future, the namespace field might be added to this list.
///
/// Alternatively, an exact spec is given by `*[sha256=01ba4719c80b6fe911b091a7c05124b64eeece964e09c058ef8f9805daca546b]`.
#[skip_serializing_none]
#[serde_as]
#[derive(Debug, Default, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct MatchSpec {
    /// The name of the package
    pub name: PackageNameMatcher,
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    pub version: Option<VersionSpec>,
    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    pub build: Option<StringMatcher>,
    /// The build number of the package
    pub build_number: Option<BuildNumberSpec>,
    /// Match the specific filename of the package
    pub file_name: Option<String>,
    /// The selected optional features of the package
    pub extras: Option<Vec<String>>,
    /// Plain string flags used to select package variants.
    pub flags: Option<Vec<StringMatcher>>,
    /// The channel of the package
    pub channel: Option<Arc<Channel>>,
    /// The subdir of the channel
    pub subdir: Option<String>,
    /// The namespace of the package (currently not used)
    pub namespace: Option<String>,
    /// The md5 hash of the package
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,
    /// The sha256 hash of the package
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
    /// The url of the package
    pub url: Option<Url>,
    /// The license of the package
    pub license: Option<String>,
    /// The license family of the package (e.g. `MIT`, `GPL`, `BSD`)
    pub license_family: Option<String>,
    /// The condition under which this match spec applies.
    pub condition: Option<MatchSpecCondition>,
    /// The track features of the package
    pub track_features: Option<Vec<String>>,
}

/// An error returned when a [`MatchSpec`] cannot be represented canonically.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum CanonicalMatchSpecError {
    /// A `when` condition contains a nested `when` condition, which `MatchSpec`
    /// grammar cannot represent.
    #[error("nested `when` conditions cannot be represented in canonical MatchSpec syntax")]
    NestedWhen,

    /// An extra cannot be represented by the canonical extras grammar.
    #[error("extra '{0}' cannot be represented in canonical MatchSpec syntax")]
    UnrepresentableExtra(String),

    /// A flag matcher cannot be represented by the canonical flags grammar.
    #[error("flag matcher '{0}' cannot be represented in canonical MatchSpec syntax")]
    UnrepresentableFlag(String),

    /// A track feature contains a delimiter used by the canonical grammar.
    #[error("track feature '{0}' cannot be represented in canonical MatchSpec syntax")]
    UnrepresentableTrackFeature(String),

    /// A package-name matcher would be parsed as a bracket field section.
    #[error("package-name matcher '{0}' cannot be represented in canonical MatchSpec syntax")]
    UnrepresentableName(String),

    /// A scalar contains both quote delimiters in forms the legacy parser
    /// cannot distinguish without changing its escape semantics.
    #[error("scalar '{0}' cannot be represented in canonical MatchSpec syntax")]
    UnrepresentableScalar(String),

    /// A build matcher would parse as a different matcher variant or value.
    #[error("build matcher '{0}' cannot be represented in canonical MatchSpec syntax")]
    UnrepresentableBuild(String),

    /// An explicit empty channel-platform selector cannot be distinguished from
    /// an omitted selector by the parser.
    #[error("an explicit empty channel-platform selector cannot be represented canonically")]
    EmptyChannelPlatforms,

    /// A condition leaf would be tokenized as a logical expression.
    #[error("condition leaf '{0}' cannot be represented in canonical MatchSpec syntax")]
    UnrepresentableConditionLeaf(String),
}

impl Display for MatchSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // The legacy dialect cannot fail; see `SpecView::fmt`.
        SpecView::from(self)
            .fmt(f, DisplayContext::LEGACY)
            .map_err(FormatError::into_fmt_error)
    }
}

impl MatchSpec {
    /// Returns the stable, square-bracket representation of this match spec.
    ///
    /// The package name comes first; every other populated field is emitted
    /// in a single bracket section, in this order: `version`, `build`,
    /// `build_number`, `fn`, `extras`, `flags`, `channel`, `subdir`,
    /// `namespace`, `md5`, `sha256`, `url`, `license`, `license_family`,
    /// `when`, and `track_features`. Unlike [`Display`], nothing is
    /// positional besides the name.
    ///
    /// States the grammar cannot represent are refused while rendering, with
    /// the error attributed to the offending field as the matching
    /// [`CanonicalMatchSpecError`] variant. No parsing happens here;
    /// round-trip fidelity is enforced by the property tests in
    /// `tests/matchspec_proptest.rs`.
    ///
    /// Channel and package URL userinfo, known token paths, query strings,
    /// and non-digest fragments are redacted, so URLs containing such data do
    /// not round-trip with exact equality.
    pub fn to_canonical_string(&self) -> Result<String, CanonicalMatchSpecError> {
        format::to_canonical_string(self)
    }

    /// Returns the repodata revision required to represent this matchspec.
    pub fn required_repodata_revision(&self) -> RepodataRevision {
        if self.extras.is_some() || self.condition.is_some() || self.flags.is_some() {
            RepodataRevision::V3
        } else {
            RepodataRevision::Legacy
        }
    }

    /// Decomposes this instance into a [`NamelessMatchSpec`] and a name.
    pub fn into_nameless(self) -> (PackageNameMatcher, NamelessMatchSpec) {
        (
            self.name,
            NamelessMatchSpec {
                version: self.version,
                build: self.build,
                build_number: self.build_number,
                file_name: self.file_name,
                extras: self.extras,
                flags: self.flags,
                channel: self.channel,
                subdir: self.subdir,
                namespace: self.namespace,
                md5: self.md5,
                sha256: self.sha256,
                url: self.url,
                license: self.license,
                license_family: self.license_family,
                condition: self.condition,
                track_features: self.track_features,
            },
        )
    }

    /// Returns whether the package is a virtual package.
    /// This is determined by the package name starting with `__`.
    /// Not having a package name is considered not virtual.
    /// Matching both virtual and non-virtual packages is considered not virtual.
    pub fn is_virtual(&self) -> bool {
        match &self.name {
            PackageNameMatcher::Exact(name) => name.as_normalized().starts_with("__"),
            PackageNameMatcher::Glob(pattern) => pattern.as_str().starts_with("__"),
            PackageNameMatcher::Regex(regex) => regex.as_str().starts_with(r"^__"),
        }
    }
}

// Enable constructing a match spec from a package name.
impl From<PackageName> for MatchSpec {
    fn from(value: PackageName) -> Self {
        Self {
            name: PackageNameMatcher::Exact(value),
            ..Default::default()
        }
    }
}

/// Similar to a [`MatchSpec`] but does not include the package name. This is useful in places
/// where the package name is already known (e.g. `foo = "3.4.1 *cuda"`)
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct NamelessMatchSpec {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    pub version: Option<VersionSpec>,
    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    pub build: Option<StringMatcher>,
    /// The build number of the package
    pub build_number: Option<BuildNumberSpec>,
    /// Match the specific filename of the package
    pub file_name: Option<String>,
    /// Optional extra dependencies to select for the package
    pub extras: Option<Vec<String>>,
    /// Plain string flags used to select package variants.
    pub flags: Option<Vec<StringMatcher>>,
    /// The channel of the package
    #[serde(deserialize_with = "deserialize_channel", default)]
    pub channel: Option<Arc<Channel>>,
    /// The subdir of the channel
    pub subdir: Option<String>,
    /// The namespace of the package (currently not used)
    pub namespace: Option<String>,
    /// The md5 hash of the package
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,
    /// The sha256 hash of the package
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
    /// The url of the package
    pub url: Option<Url>,
    /// The license of the package
    pub license: Option<String>,
    /// The license family of the package (e.g. `MIT`, `GPL`, `BSD`)
    pub license_family: Option<String>,
    /// The condition under which this match spec applies.
    pub condition: Option<MatchSpecCondition>,
    /// The track features of the package
    pub track_features: Option<Vec<String>>,
}

impl Display for NamelessMatchSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // The legacy dialect cannot fail; see `SpecView::fmt`.
        SpecView::from(self)
            .fmt(f, DisplayContext::LEGACY)
            .map_err(FormatError::into_fmt_error)
    }
}

impl From<MatchSpec> for NamelessMatchSpec {
    fn from(spec: MatchSpec) -> Self {
        Self {
            version: spec.version,
            build: spec.build,
            build_number: spec.build_number,
            file_name: spec.file_name,
            extras: spec.extras,
            flags: spec.flags,
            channel: spec.channel,
            subdir: spec.subdir,
            namespace: spec.namespace,
            md5: spec.md5,
            sha256: spec.sha256,
            url: spec.url,
            license: spec.license,
            license_family: spec.license_family,
            condition: spec.condition,
            track_features: spec.track_features,
        }
    }
}

impl MatchSpec {
    /// Constructs a [`MatchSpec`] from a [`NamelessMatchSpec`] and a name.
    pub fn from_nameless(spec: NamelessMatchSpec, name: PackageNameMatcher) -> Self {
        Self {
            name,
            version: spec.version,
            build: spec.build,
            build_number: spec.build_number,
            file_name: spec.file_name,
            extras: spec.extras,
            flags: spec.flags,
            channel: spec.channel,
            subdir: spec.subdir,
            namespace: spec.namespace,
            md5: spec.md5,
            sha256: spec.sha256,
            url: spec.url,
            license: spec.license,
            license_family: spec.license_family,
            condition: spec.condition,
            track_features: spec.track_features,
        }
    }
}

/// Deserialize channel from string
/// TODO: This should be refactored so that the front ends are the one setting the channel config,
/// and rattler only takes care of the url.
fn deserialize_channel<'de, D>(deserializer: D) -> Result<Option<Arc<Channel>>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;

    match s {
        Some(str_val) => {
            // The root directory only resolves relative path channels. With
            // an empty-root fallback those fail with an error instead of
            // panicking when the current directory is unavailable.
            let config =
                ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap_or_default());

            Channel::from_str(str_val, &config)
                .map(|channel| Some(Arc::new(channel)))
                .map_err(serde::de::Error::custom)
        }
        None => Ok(None),
    }
}

/// A trait that defines the behavior of matching a spec against a record.
pub trait Matches<T> {
    /// Match a [`MatchSpec`] against a record.
    /// Matching it to a record means that the record is valid for the spec.
    fn matches(&self, other: &T) -> bool;
}

impl Matches<PackageRecord> for NamelessMatchSpec {
    /// Match a [`NamelessMatchSpec`] against a [`PackageRecord`]
    fn matches(&self, other: &PackageRecord) -> bool {
        if let Some(spec) = self.version.as_ref()
            && !spec.matches(&other.version)
        {
            return false;
        }

        if let Some(build_string) = self.build.as_ref()
            && !build_string.matches(&other.build)
        {
            return false;
        }

        if let Some(build_number) = self.build_number.as_ref()
            && !build_number.matches(&other.build_number)
        {
            return false;
        }

        if let Some(md5_spec) = self.md5.as_ref()
            && Some(md5_spec) != other.md5.as_ref()
        {
            return false;
        }

        if let Some(sha256_spec) = self.sha256.as_ref()
            && Some(sha256_spec) != other.sha256.as_ref()
        {
            return false;
        }

        if let Some(license) = self.license.as_ref()
            && Some(license) != other.license.as_ref()
        {
            return false;
        }

        if let Some(license_family) = self.license_family.as_ref()
            && Some(license_family) != other.license_family.as_ref()
        {
            return false;
        }

        if let Some(track_features) = self.track_features.as_ref() {
            for feature in track_features {
                if !other.track_features.contains(feature) {
                    return false;
                }
            }
        }

        if let Some(flags) = self.flags.as_ref() {
            for flag in flags {
                if !other
                    .flags
                    .iter()
                    .any(|record_flag| flag.matches(record_flag.as_str()))
                {
                    return false;
                }
            }
        }

        true
    }
}

impl Matches<PackageRecord> for MatchSpec {
    /// Match a [`MatchSpec`] against a [`PackageRecord`]
    fn matches(&self, other: &PackageRecord) -> bool {
        if !self.name.matches(&other.name) {
            return false;
        }

        if let Some(spec) = self.version.as_ref()
            && !spec.matches(&other.version)
        {
            return false;
        }

        if let Some(build_string) = self.build.as_ref()
            && !build_string.matches(&other.build)
        {
            return false;
        }

        if let Some(build_number) = self.build_number.as_ref()
            && !build_number.matches(&other.build_number)
        {
            return false;
        }

        if let Some(md5_spec) = self.md5.as_ref()
            && Some(md5_spec) != other.md5.as_ref()
        {
            return false;
        }

        if let Some(sha256_spec) = self.sha256.as_ref()
            && Some(sha256_spec) != other.sha256.as_ref()
        {
            return false;
        }

        if let Some(license) = self.license.as_ref()
            && Some(license) != other.license.as_ref()
        {
            return false;
        }

        if let Some(license_family) = self.license_family.as_ref()
            && Some(license_family) != other.license_family.as_ref()
        {
            return false;
        }

        if let Some(track_features) = self.track_features.as_ref() {
            for feature in track_features {
                if !other.track_features.contains(feature) {
                    return false;
                }
            }
        }

        if let Some(flags) = self.flags.as_ref() {
            for flag in flags {
                if !other
                    .flags
                    .iter()
                    .any(|record_flag| flag.matches(record_flag.as_str()))
                {
                    return false;
                }
            }
        }

        true
    }
}

impl Matches<RepoDataRecord> for MatchSpec {
    /// Match a [`MatchSpec`] against a [`RepoDataRecord`]
    fn matches(&self, other: &RepoDataRecord) -> bool {
        if let Some(url_spec) = self.url.as_ref()
            && url_spec != &other.url
        {
            return false;
        }

        if !self.matches(&other.package_record) {
            return false;
        }

        true
    }
}

impl Matches<RepoDataRecord> for NamelessMatchSpec {
    /// Match a [`NamelessMatchSpec`] against a [`RepoDataRecord`]
    fn matches(&self, other: &RepoDataRecord) -> bool {
        if let Some(url_spec) = self.url.as_ref()
            && url_spec != &other.url
        {
            return false;
        }

        if !self.matches(&other.package_record) {
            return false;
        }

        true
    }
}

impl Matches<GenericVirtualPackage> for MatchSpec {
    /// Match a [`MatchSpec`] against a [`GenericVirtualPackage`]
    fn matches(&self, other: &GenericVirtualPackage) -> bool {
        if !self.name.matches(&other.name) {
            return false;
        }

        if let Some(spec) = self.version.as_ref()
            && !spec.matches(&other.version)
        {
            return false;
        }

        if let Some(build_string) = self.build.as_ref()
            && !build_string.matches(&other.build_string)
        {
            return false;
        }
        true
    }
}

/// Convert a URL to a [`MatchSpec`]. This parses the URL and adds a `#sha256:...` or `md5=...`
/// from the fragment of the URL if it exists.
impl TryFrom<Url> for MatchSpec {
    type Error = MatchSpecUrlError;

    fn try_from(value: Url) -> Result<Self, Self::Error> {
        let mut spec = MatchSpec::default();
        let mut url_without_fragment = value.clone();
        url_without_fragment.set_fragment(None);
        spec.url = Some(url_without_fragment);

        // Handle URL fragment for checksums
        if let Some(fragment) = value.fragment() {
            if fragment.starts_with("sha256:") {
                let sha256 = fragment.trim_start_matches("sha256:");
                spec.sha256 = Some(
                    parse_digest_from_hex::<Sha256>(sha256)
                        .ok_or(MatchSpecUrlError::InvalidSha256(fragment.to_string()))?,
                );
            } else if !fragment.is_empty() {
                spec.md5 = Some(
                    parse_digest_from_hex::<Md5>(fragment)
                        .ok_or(MatchSpecUrlError::InvalidMd5(fragment.to_string()))?,
                );
            }
        }

        // Parse the filename from the URL and extract package information
        let filename = value
            .path_segments()
            .and_then(Iterator::last)
            .ok_or(MatchSpecUrlError::MissingFilename)?;

        let archive_identifier = CondaArchiveIdentifier::try_from_filename(filename)
            .ok_or(MatchSpecUrlError::InvalidFilename(filename.to_string()))?;
        spec.name = archive_identifier
            .identifier
            .name
            .parse::<PackageNameMatcher>()
            .map_err(|e| MatchSpecUrlError::InvalidPackageName(e.to_string()))?;
        Ok(spec)
    }
}

/// Errors that can occur when converting a URL to a `MatchSpec`
#[derive(Debug, thiserror::Error)]
pub enum MatchSpecUrlError {
    /// The URL is missing a conda package filename
    #[error("Missing filename in URL")]
    MissingFilename,

    /// The URL fragment is not a valid SHA256 digest
    #[error("Invalid SHA256 digest: {0}")]
    InvalidSha256(String),

    /// The URL fragment is not a valid MD5 digest
    #[error("Invalid MD5 digest: {0}")]
    InvalidMd5(String),

    /// The filename is not a valid conda package filename
    #[error("Invalid filename: {0}")]
    InvalidFilename(String),

    /// The package name is not a valid conda package name
    #[error("Invalid package name: {0}")]
    InvalidPackageName(String),
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use rstest::rstest;
    use std::str::FromStr;

    use rattler_digest::{Md5, Sha256, parse_digest_from_hex};

    use crate::{
        Flag, MatchSpec, NamelessMatchSpec, PackageName, PackageRecord, ParseMatchSpecError,
        ParseMatchSpecOptions, ParseStrictness::*, RepoDataRecord, RepodataRevision, StringMatcher,
        Version, match_spec::Matches, package::DistArchiveIdentifier,
        parse_mode::ParseStrictnessWithNameMatcher,
    };
    use insta::assert_snapshot;
    use std::hash::{Hash, Hasher};

    #[test]
    fn test_matchspec_format_eq() {
        let spec = MatchSpec::from_str("conda-forge::mamba[version==1.0, sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97, md5=dede6252c964db3f3e41c7d30d07f6bf]", Strict).unwrap();
        let spec_as_string = spec.to_string();
        let rebuild_spec = MatchSpec::from_str(&spec_as_string, Strict).unwrap();

        assert_eq!(spec, rebuild_spec);
    }

    #[test]
    fn test_name_asterisk() {
        use crate::match_spec::package_name_matcher::PackageNameMatcher;
        use crate::{MatchSpec, ParseMatchSpecOptions, ParseStrictness::Lenient, VersionSpec};

        let options = ParseMatchSpecOptions::from(Lenient).with_exact_names_only(false);

        let spec = MatchSpec::from_str("*[license=MIT]", options).unwrap();
        assert_eq!(spec.name, PackageNameMatcher::from_str("*").unwrap());
        assert_eq!(spec.license, Some("MIT".to_string()));

        let spec = MatchSpec::from_str("* >=1.0", options).unwrap();
        assert_eq!(spec.name, PackageNameMatcher::from_str("*").unwrap());
        assert_eq!(
            spec.version,
            Some(VersionSpec::from_str(">=1.0", Lenient).unwrap())
        );
    }

    #[test]
    fn test_name_asterisk_edge_cases() {
        use crate::match_spec::package_name_matcher::PackageNameMatcher;
        use crate::{
            MatchSpec, ParseMatchSpecError, ParseMatchSpecOptions, ParseStrictness::Strict,
            VersionSpec,
        };

        // In strict mode (exact_names_only = true), a standalone `*` should be rejected.
        let strict_spec = MatchSpec::from_str("*", Strict);
        match strict_spec {
            Err(ParseMatchSpecError::OnlyExactPackageNameMatchersAllowedGlob(g)) => {
                assert_eq!(g, "*");
            }
            other => panic!("Expected glob rejection in strict mode, got: {other:?}"),
        }

        // `*` as a glob inside a complex spec string with channel, subdir, version, build
        let options = ParseMatchSpecOptions::from(Strict).with_exact_names_only(false);
        let spec = MatchSpec::from_str(
            "conda-forge/linux-64::*[version=\">=2.0\", build=\"*_cpython\"]",
            options,
        )
        .unwrap();

        assert_eq!(spec.name, PackageNameMatcher::from_str("*").unwrap());
        assert_eq!(spec.channel.unwrap().name(), "conda-forge");
        assert_eq!(spec.subdir, Some("linux-64".to_string()));
        assert_eq!(
            spec.version,
            Some(VersionSpec::from_str(">=2.0", Strict).unwrap())
        );
        assert!(spec.build.is_some());
    }

    #[test]
    fn test_nameless_matchspec_format_eq() {
        let spec = NamelessMatchSpec::from_str("*[version==1.0, sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97, md5=dede6252c964db3f3e41c7d30d07f6bf]", Lenient).unwrap();
        let spec_as_string = spec.to_string();
        let rebuild_spec = NamelessMatchSpec::from_str(&spec_as_string, Strict).unwrap();

        assert_eq!(spec, rebuild_spec);
    }

    #[test]
    fn test_hash_match() {
        let spec1 = MatchSpec::from_str("tensorflow 2.6.*", Strict).unwrap();
        let spec2 = MatchSpec::from_str("tensorflow 2.6.*", Strict).unwrap();
        assert_eq!(spec1, spec2);

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        spec1.hash(&mut hasher);
        let hash1 = hasher.finish();

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        spec2.hash(&mut hasher);
        let hash2 = hasher.finish();

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_no_match() {
        let spec1 = MatchSpec::from_str("tensorflow 2.6.0.*", Strict).unwrap();
        let spec2 = MatchSpec::from_str("tensorflow 2.6.*", Strict).unwrap();
        assert_ne!(spec1, spec2);

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        spec1.hash(&mut hasher);
        let hash1 = hasher.finish();

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        spec2.hash(&mut hasher);
        let hash2 = hasher.finish();

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_digest_match() {
        let record = PackageRecord {
            sha256: parse_digest_from_hex::<Sha256>(
                "f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97",
            ),
            md5: parse_digest_from_hex::<Md5>("dede6252c964db3f3e41c7d30d07f6bf"),
            ..PackageRecord::new(
                PackageName::new_unchecked("mamba"),
                Version::from_str("1.0").unwrap(),
                String::from("foo_bar_py310_1"),
            )
        };

        let spec = MatchSpec::from_str("mamba[version==1.0, sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97]", Strict).unwrap();
        assert!(!spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[version==1.0, sha256=f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97]", Strict).unwrap();
        assert!(spec.matches(&record));

        let spec = MatchSpec::from_str(
            "mamba[version==1.0, md5=aaaa6252c964db3f3e41c7d30d07f6bf]",
            Strict,
        )
        .unwrap();
        assert!(!spec.matches(&record));

        let spec = MatchSpec::from_str(
            "mamba[version==1.0, md5=dede6252c964db3f3e41c7d30d07f6bf]",
            Strict,
        )
        .unwrap();
        assert!(spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[version==1.0, md5=dede6252c964db3f3e41c7d30d07f6bf, sha256=f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97]", Strict).unwrap();
        assert!(spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[version==1.0, md5=dede6252c964db3f3e41c7d30d07f6bf, sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97]", Strict).unwrap();
        assert!(!spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[build=*py310_1]", Strict).unwrap();
        assert!(spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[build=*py310*]", Strict).unwrap();
        assert!(spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[build=*py39*]", Strict).unwrap();
        assert!(!spec.matches(&record));

        let spec = MatchSpec::from_str("mamba * [build=*py310*]", Strict).unwrap();
        assert!(spec.matches(&record));

        let spec = MatchSpec::from_str("mamba *[build=*py39*]", Strict).unwrap();
        assert!(!spec.matches(&record));
        assert!(spec.build == Some(StringMatcher::from_str("*py39*").unwrap()));

        let spec = MatchSpec::from_str("mamba * [build=*py39*]", Strict).unwrap();
        println!("Build: {:?}", spec.build);
        assert!(!spec.matches(&record));
    }

    #[test]
    fn test_flags_match() {
        let options = ParseMatchSpecOptions::strict().with_repodata_revision(RepodataRevision::V3);
        let spec = MatchSpec::from_str("mamba[flags=[cuda, blas:*]]", options).unwrap();

        assert_eq!(spec.required_repodata_revision(), RepodataRevision::V3);
        assert_eq!(
            spec.flags,
            Some(vec![
                StringMatcher::from_str("cuda").unwrap(),
                StringMatcher::from_str("blas:*").unwrap(),
            ])
        );
        assert_eq!(spec.to_string(), "mamba[flags=[cuda, blas:*]]");
        assert_eq!(
            MatchSpec::from_str(&spec.to_string(), options).unwrap(),
            spec
        );

        let matching_record = PackageRecord {
            flags: vec![Flag::new_unchecked("cuda"), Flag::new_unchecked("blas:mkl")],
            ..PackageRecord::new(
                PackageName::new_unchecked("mamba"),
                Version::from_str("1.0").unwrap(),
                String::from("foo_bar_py310_1"),
            )
        };
        assert!(spec.matches(&matching_record));

        let missing_blas_record = PackageRecord {
            flags: vec![Flag::new_unchecked("cuda")],
            ..matching_record.clone()
        };
        assert!(!spec.matches(&missing_blas_record));

        let legacy_err = MatchSpec::from_str("mamba[flags=[cuda]]", Strict).unwrap_err();
        assert_eq!(
            legacy_err,
            ParseMatchSpecError::InvalidBracketKey("flags".to_string())
        );
    }

    #[test]
    fn precedence_version_build() {
        let spec =
            MatchSpec::from_str("foo 3.0.* [version=1.2.3, build='foobar']", Lenient).unwrap();
        assert_eq!(spec.version.unwrap(), "1.2.3".parse().unwrap());
        assert_eq!(spec.build.unwrap(), "foobar".parse().unwrap());

        let spec = MatchSpec::from_str("foo 3.0.* abcdef[build='foobar', version=1.2.3]", Lenient)
            .unwrap();
        assert_eq!(spec.build.unwrap(), "foobar".parse().unwrap());
        assert_eq!(spec.version.unwrap(), "1.2.3".parse().unwrap());

        let spec =
            NamelessMatchSpec::from_str("3.0.* [version=1.2.3, build='foobar']", Lenient).unwrap();
        assert_eq!(spec.version.unwrap(), "1.2.3".parse().unwrap());
        assert_eq!(spec.build.unwrap(), "foobar".parse().unwrap());

        let spec =
            NamelessMatchSpec::from_str("3.0.* abcdef[build='foobar', version=1.2.3]", Lenient)
                .unwrap();
        assert_eq!(spec.build.unwrap(), "foobar".parse().unwrap());
        assert_eq!(spec.version.unwrap(), "1.2.3".parse().unwrap());
    }

    #[test]
    fn strict_parsing_multiple_values() {
        let spec = NamelessMatchSpec::from_str("3.0.* [version=1.2.3]", Strict);
        assert!(spec.is_err());

        let spec = NamelessMatchSpec::from_str("3.0.* foo[build='foobar']", Strict);
        assert!(spec.is_err());

        let spec = NamelessMatchSpec::from_str(
            "3.0.* [build=baz, fn='/home/bla.tar.bz2' build='foobar']",
            Strict,
        );
        assert!(spec.is_err());

        let spec = MatchSpec::from_str("foo 3.0.* [version=1.2.3]", Strict);
        assert!(spec.is_err());

        let spec = MatchSpec::from_str("foo 3.0.* foo[build='foobar']", Strict);
        assert!(spec.is_err());
        assert!(
            spec.unwrap_err()
                .to_string()
                .contains("multiple values for: build")
        );

        let spec = MatchSpec::from_str(
            "foo 3.0.* [build=baz, fn='/home/foo.tar.bz2', build='foobar']",
            Strict,
        );
        assert!(spec.is_err());
        assert!(
            spec.unwrap_err()
                .to_string()
                .contains("multiple values for: build")
        );
    }

    #[test]
    fn test_layered_matches() {
        let repodata_record = RepoDataRecord {
            package_record: PackageRecord::new(
                PackageName::new_unchecked("mamba"),
                Version::from_str("1.0").unwrap(),
                String::from(""),
            ),
            identifier: "mamba-1.0-py37_0.conda"
                .parse::<DistArchiveIdentifier>()
                .unwrap(),
            url: url::Url::parse("https://mamba.io/mamba-1.0-py37_0.conda").unwrap(),
            channel: Some(String::from("mamba")),
        };
        let package_record = repodata_record.clone().package_record;

        // Test with basic spec
        let match_spec = MatchSpec::from_str("mamba[version==1.0]", Strict).unwrap();
        let nameless_spec = match_spec.clone().into_nameless().1;

        assert!(match_spec.matches(&repodata_record));
        assert!(match_spec.matches(&package_record));
        assert!(nameless_spec.matches(&repodata_record));
        assert!(nameless_spec.matches(&package_record));

        // Test with url spec
        let match_spec =
            MatchSpec::from_str("https://mamba.io/mamba-1.0-py37_0.conda", Strict).unwrap();
        let nameless_spec = match_spec.clone().into_nameless().1;

        assert!(match_spec.matches(&repodata_record));
        assert!(match_spec.matches(&package_record));
        assert!(nameless_spec.matches(&repodata_record));
        assert!(nameless_spec.matches(&package_record));
    }

    #[test]
    fn test_field_matches() {
        let mut repodata_record = RepoDataRecord {
            package_record: PackageRecord::new(
                PackageName::new_unchecked("mamba"),
                Version::from_str("1.0").unwrap(),
                String::from(""),
            ),
            identifier: "mamba-1.0-py37_0.conda"
                .parse::<DistArchiveIdentifier>()
                .unwrap(),
            url: url::Url::parse("https://mamba.io/mamba-1.0-py37_0.conda").unwrap(),
            channel: Some(String::from("mamba")),
        };
        repodata_record.package_record.license = Some("BSD-3-Clause".into());
        let package_record = repodata_record.clone().package_record;

        let match_spec = MatchSpec::from_str("mamba[license=BSD-3-Clause]", Strict).unwrap();
        let nameless_spec = match_spec.clone().into_nameless().1;
        assert!(match_spec.matches(&repodata_record));
        assert!(match_spec.matches(&package_record));
        assert!(nameless_spec.matches(&repodata_record));
        assert!(nameless_spec.matches(&package_record));

        let match_spec = MatchSpec::from_str("mamba[license=MIT]", Strict).unwrap();
        let nameless_spec = match_spec.clone().into_nameless().1;
        assert!(!match_spec.matches(&repodata_record));
        assert!(!match_spec.matches(&package_record));
        assert!(!nameless_spec.matches(&repodata_record));
        assert!(!nameless_spec.matches(&package_record));

        let repodata_record_no_license = RepoDataRecord {
            package_record: PackageRecord::new(
                PackageName::new_unchecked("mamba"),
                Version::from_str("1.0").unwrap(),
                String::from(""),
            ),
            identifier: "mamba-1.0-py37_0.conda"
                .parse::<DistArchiveIdentifier>()
                .unwrap(),
            url: url::Url::parse("https://mamba.io/mamba-1.0-py37_0.conda").unwrap(),
            channel: Some(String::from("mamba")),
        };
        let package_record_no_license = repodata_record_no_license.clone().package_record;
        assert!(!match_spec.matches(&repodata_record_no_license));
        assert!(!match_spec.matches(&package_record_no_license));
        assert!(!nameless_spec.matches(&repodata_record_no_license));
        assert!(!nameless_spec.matches(&package_record_no_license));
    }

    #[test]
    fn test_serialize_matchspec() {
        let specs = [
            "mamba 1.0.* py37_0",
            "conda-forge::pytest[version='==1.0', sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97, md5=dede6252c964db3f3e41c7d30d07f6bf]",
            "conda-forge/linux-64::pytest",
            "conda-forge/linux-64::pytest[version=1.0.*]",
            "conda-forge/linux-64::pytest[version=1.0.*, build=py37_0, license=MIT]",
            "conda-forge/linux-64::pytest ==1.2.3",
        ];

        assert_snapshot!(
            specs
                .into_iter()
                .map(|s| MatchSpec::from_str(s, Strict).unwrap())
                .map(|s| s.to_string())
                .format("\n")
                .to_string()
        );
    }

    #[test]
    fn test_serialize_json_matchspec() {
        let specs = [
            "mamba 1.0.* py37_0",
            "conda-forge::pytest[version='==1.0', sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97, md5=dede6252c964db3f3e41c7d30d07f6bf]",
            "conda-forge/linux-64::pytest",
            "conda-forge/linux-64::pytest[version=1.0.*]",
            "conda-forge/linux-64::pytest[version=1.0.*, build=py37_0]",
            "conda-forge/linux-64::pytest ==1.2.3",
        ];

        assert_snapshot!(
            specs
                .into_iter()
                .map(|s| MatchSpec::from_str(s, Strict).unwrap())
                .map(|s| serde_json::to_string(&s).unwrap())
                .format("\n")
                .to_string()
        );
    }

    #[rstest]
    #[case("foo >=1.0 py37_0", true)]
    #[case("foo >=1.0 py37*", true)]
    #[case("foo 1.0.* py38*", false)]
    #[case("foo * py37_1", false)]
    #[case("foo ==1.0", true)]
    #[case("foo >=2.0", false)]
    #[case("foo >=1.0", true)]
    #[case("foo", true)]
    #[case("bar", false)]
    fn test_match_generic_virtual_package(#[case] spec_str: &str, #[case] expected: bool) {
        let virtual_package = crate::GenericVirtualPackage {
            name: PackageName::new_unchecked("foo"),
            version: Version::from_str("1.0").unwrap(),
            build_string: String::from("py37_0"),
        };

        let spec = MatchSpec::from_str(spec_str, Strict).unwrap();
        assert_eq!(spec.matches(&virtual_package), expected);
    }

    #[test]
    fn test_is_virtual() {
        let spec = MatchSpec::from_str("non_virtual_name", Strict).unwrap();
        assert!(!spec.is_virtual());

        let spec = MatchSpec::from_str("__virtual_name", Strict).unwrap();
        assert!(spec.is_virtual());

        let spec = MatchSpec::from_str("non_virtual_name >=12", Strict).unwrap();
        assert!(!spec.is_virtual());

        let spec = MatchSpec::from_str("__virtual_name >=12", Strict).unwrap();
        assert!(spec.is_virtual());

        let spec = MatchSpec::from_nameless(
            NamelessMatchSpec::from_str(">=12", Strict).unwrap(),
            "dummy".parse().unwrap(),
        );
        assert!(!spec.is_virtual());

        let spec = MatchSpec::from_str(
            "__virtual_glob*",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();
        assert!(spec.is_virtual());

        let spec = MatchSpec::from_str(
            "^__virtual_regex.*$",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();
        assert!(spec.is_virtual());

        // technically, these can also match virtual packages like `__spec_with_glob`
        // but as this also matches packages that are not virtual, `is_virtual` should be `false`
        let spec = MatchSpec::from_str(
            "*spec_with_glob",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();
        assert!(!spec.is_virtual());

        let spec = MatchSpec::from_str(
            "^.*spec_with_regex$",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();
        assert!(!spec.is_virtual());
    }

    #[test]
    fn test_glob_in_name() {
        let spec = MatchSpec::from_str(
            "foo* >=12",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();
        assert!(spec.matches(&PackageRecord::new(
            PackageName::from_str("foo").unwrap(),
            Version::from_str("13.0").unwrap(),
            String::from(""),
        )));
        assert!(!spec.matches(&PackageRecord::new(
            PackageName::from_str("foo").unwrap(),
            Version::from_str("11.0").unwrap(),
            String::from(""),
        )));
        assert!(spec.matches(&PackageRecord::new(
            PackageName::from_str("foo-bar").unwrap(),
            Version::from_str("12.0").unwrap(),
            String::from(""),
        )));

        let spec = MatchSpec::from_str(
            "foo* >=12[license=MIT]",
            ParseStrictnessWithNameMatcher {
                parse_strictness: Strict,
                exact_names_only: false,
            },
        )
        .unwrap();
        assert!(!spec.matches(&PackageRecord::new(
            PackageName::from_str("foo-bar").unwrap(),
            Version::from_str("12.0").unwrap(),
            String::from(""),
        )));
        assert!(spec.matches(&{
            let mut record = PackageRecord::new(
                PackageName::from_str("foo-bar").unwrap(),
                Version::from_str("12.0").unwrap(),
                String::from(""),
            );
            record.license = Some("MIT".into());
            record
        }));
    }

    #[test]
    fn test_allow_exact_names_only() {
        let err = MatchSpec::from_str("foo* >=12[license=MIT]", Strict).unwrap_err();
        assert_eq!(
            err,
            ParseMatchSpecError::OnlyExactPackageNameMatchersAllowedGlob("foo*".to_string())
        );
    }
}
