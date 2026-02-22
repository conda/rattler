//! Query language for conda packages.
use crate::match_spec::condition::MatchSpecCondition;
use crate::package::CondaArchiveIdentifier;
use crate::{
    build_spec::BuildNumberSpec, GenericVirtualPackage, PackageName, PackageRecord, RepoDataRecord,
    VersionSpec,
};
use itertools::Itertools;
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
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
/// Match a given string either by exact match, glob or regex
pub mod matcher;
/// Match package names either by exact match, glob or regex
pub mod package_name_matcher;
/// Parse a match spec from a string
pub mod parse;

use matcher::StringMatcher;
use package_name_matcher::PackageNameMatcher;
use parse::escape_bracket_value;

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
/// A series of rules are now followed for creating the canonical string representation of a
/// `MatchSpec` instance. The canonical string representation can generically be
/// represented by
///
/// (channel(/subdir):(namespace):)name(version(build))[key1=value1,key2=value2]
///
/// where `()` indicate optional fields.
///
/// The rules for constructing a canonical string representation are:
///
/// 1. `name` (i.e. "package name") is required, but its value can be '*'. Its position is always
///    outside the key-value brackets.
/// 2. If `version` is an exact version, it goes outside the key-value brackets and is prepended
///    by `==`. If `version` is a "fuzzy" value (e.g. `1.11.*`), it goes outside the key-value
///    brackets with the `.*` left off and is prepended by `=`. Otherwise `version` is included
///    inside key-value brackets.
/// 3. If `version` is an exact version, and `build` is an exact value, `build` goes outside
///    key-value brackets prepended by a `=`.  Otherwise, `build` goes inside key-value brackets.
///    `build_string` is an alias for `build`.
/// 4. The `namespace` position is being held for a future feature. It is currently ignored.
/// 5. If `channel` is included and is an exact value, a `::` separator is used between `channel`
///    and `name`.  `channel` can either be a canonical channel name or a channel url.  In the
///    canonical string representation, the canonical channel name will always be used.
/// 6. If `channel` is an exact value and `subdir` is an exact value, `subdir` is appended to
///    `channel` with a `/` separator.  Otherwise, `subdir` is included in the key-value brackets.
/// 7. Key-value brackets can be delimited by comma, space, or comma+space.  Value can optionally
///    be wrapped in single or double quotes, but must be wrapped if `value` contains a comma,
///    space, or equal sign.  The canonical format uses comma delimiters and single quotes.
/// 8. When constructing a `MatchSpec` instance from a string, any key-value pair given
///    inside the key-value brackets overrides any matching parameter given outside the brackets.
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
    /// The condition under which this match spec applies.
    pub condition: Option<MatchSpecCondition>,
    /// The track features of the package
    pub track_features: Option<Vec<String>>,
}

impl Display for MatchSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(channel) = &self.channel {
            let name = channel.name();
            write!(f, "{name}")?;

            if let Some(subdir) = &self.subdir {
                write!(f, "/{subdir}")?;
            }
        }

        if let Some(namespace) = &self.namespace {
            write!(f, ":{namespace}:")?;
        } else if self.channel.is_some() || self.subdir.is_some() {
            write!(f, "::")?;
        }

        write!(f, "{}", self.name)?;

        if let Some(version) = &self.version {
            write!(f, " {version}")?;
        }

        if let Some(build) = &self.build {
            write!(f, " {build}")?;
        }

        let mut keys = Vec::new();

        if let Some(extras) = &self.extras {
            keys.push(format!("extras=[{}]", extras.iter().format(", ")));
        }

        if let Some(md5) = &self.md5 {
            keys.push(format!("md5=\"{md5:x}\""));
        }

        if let Some(sha256) = &self.sha256 {
            keys.push(format!("sha256=\"{sha256:x}\""));
        }

        if let Some(build_number) = &self.build_number {
            keys.push(format!("build_number=\"{build_number}\""));
        }

        if let Some(file_name) = &self.file_name {
            keys.push(format!("fn=\"{file_name}\""));
        }

        if let Some(url) = &self.url {
            keys.push(format!("url=\"{url}\""));
        }

        if let Some(license) = &self.license {
            keys.push(format!("license=\"{license}\""));
        }

        if let Some(track_features) = &self.track_features {
            keys.push(format!(
                "track_features=\"{}\"",
                track_features.iter().format(" ")
            ));
        }

        if let Some(condition) = &self.condition {
            let condition_str = condition.to_string();
            keys.push(format!("when=\"{}\"", escape_bracket_value(&condition_str)));
        }

        if !keys.is_empty() {
            write!(f, "[{}]", keys.join(", "))?;
        }

        Ok(())
    }
}

impl MatchSpec {
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
                channel: self.channel,
                subdir: self.subdir,
                namespace: self.namespace,
                md5: self.md5,
                sha256: self.sha256,
                url: self.url,
                license: self.license,
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
    /// The condition under which this match spec applies.
    pub condition: Option<MatchSpecCondition>,
    /// The track features of the package
    pub track_features: Option<Vec<String>>,
}

impl Display for NamelessMatchSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.version {
            Some(version) => write!(f, "{version}")?,
            None => write!(f, "*")?,
        }

        if let Some(build) = &self.build {
            write!(f, " {build}")?;
        }

        let mut keys = Vec::new();

        if let Some(md5) = &self.md5 {
            keys.push(format!("md5=\"{md5:x}\""));
        }

        if let Some(sha256) = &self.sha256 {
            keys.push(format!("sha256=\"{sha256:x}\""));
        }

        if let Some(condition) = &self.condition {
            let condition_str = condition.to_string();
            keys.push(format!("when=\"{}\"", escape_bracket_value(&condition_str)));
        }

        if !keys.is_empty() {
            write!(f, "[{}]", keys.join(", "))?;
        }

        Ok(())
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
            channel: spec.channel,
            subdir: spec.subdir,
            namespace: spec.namespace,
            md5: spec.md5,
            sha256: spec.sha256,
            url: spec.url,
            license: spec.license,
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
            channel: spec.channel,
            subdir: spec.subdir,
            namespace: spec.namespace,
            md5: spec.md5,
            sha256: spec.sha256,
            url: spec.url,
            license: spec.license,
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
            let config = ChannelConfig::default_with_root_dir(
                std::env::current_dir().expect("Could not determine current directory"),
            );

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
        if let Some(spec) = self.version.as_ref() {
            if !spec.matches(&other.version) {
                return false;
            }
        }

        if let Some(build_string) = self.build.as_ref() {
            if !build_string.matches(&other.build) {
                return false;
            }
        }

        if let Some(build_number) = self.build_number.as_ref() {
            if !build_number.matches(&other.build_number) {
                return false;
            }
        }

        if let Some(md5_spec) = self.md5.as_ref() {
            if Some(md5_spec) != other.md5.as_ref() {
                return false;
            }
        }

        if let Some(sha256_spec) = self.sha256.as_ref() {
            if Some(sha256_spec) != other.sha256.as_ref() {
                return false;
            }
        }

        if let Some(license) = self.license.as_ref() {
            if Some(license) != other.license.as_ref() {
                return false;
            }
        }

        if let Some(track_features) = self.track_features.as_ref() {
            for feature in track_features {
                if !other.track_features.contains(feature) {
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

        if let Some(spec) = self.version.as_ref() {
            if !spec.matches(&other.version) {
                return false;
            }
        }

        if let Some(build_string) = self.build.as_ref() {
            if !build_string.matches(&other.build) {
                return false;
            }
        }

        if let Some(build_number) = self.build_number.as_ref() {
            if !build_number.matches(&other.build_number) {
                return false;
            }
        }

        if let Some(md5_spec) = self.md5.as_ref() {
            if Some(md5_spec) != other.md5.as_ref() {
                return false;
            }
        }

        if let Some(sha256_spec) = self.sha256.as_ref() {
            if Some(sha256_spec) != other.sha256.as_ref() {
                return false;
            }
        }

        if let Some(license) = self.license.as_ref() {
            if Some(license) != other.license.as_ref() {
                return false;
            }
        }

        if let Some(track_features) = self.track_features.as_ref() {
            for feature in track_features {
                if !other.track_features.contains(feature) {
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
        if let Some(url_spec) = self.url.as_ref() {
            if url_spec != &other.url {
                return false;
            }
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
        if let Some(url_spec) = self.url.as_ref() {
            if url_spec != &other.url {
                return false;
            }
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
        let name = &self.name;
        if !name.matches(&other.name) {
            return false;
        }

        if let Some(spec) = self.version.as_ref() {
            if !spec.matches(&other.version) {
                return false;
            }
        }

        if let Some(build_string) = self.build.as_ref() {
            if !build_string.matches(&other.build_string) {
                return false;
            }
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

    use rattler_digest::{parse_digest_from_hex, Md5, Sha256};

    use crate::{
        match_spec::Matches, package::DistArchiveIdentifier,
        parse_mode::ParseStrictnessWithNameMatcher, MatchSpec, NamelessMatchSpec, PackageName,
        PackageRecord, ParseMatchSpecError, ParseStrictness::*, RepoDataRecord, StringMatcher,
        Version,
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

        // Explicitly configure the parser to allow glob matching
        let options = ParseMatchSpecOptions::from(Lenient).with_exact_names_only(false);

        // Test that MatchSpec can be created with an asterisk as the package name
        let spec = MatchSpec::from_str("*[license=MIT]", options).unwrap();

        // It should now correctly be identified as a Glob instead of None!
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

        // EDGE CASE 1: The Security Boundary
        // In Strict mode (exact_names_only = true), a standalone `*` SHOULD be rejected.
        // The previous hack used to bypass this. The new architecture catches it.
        let strict_spec = MatchSpec::from_str("*", Strict);
        match strict_spec {
            Err(ParseMatchSpecError::OnlyExactPackageNameMatchersAllowedGlob(g)) => {
                assert_eq!(g, "*");
            }
            other => panic!("Strict mode failed to block the glob! Got: {other:?}"),
        }

        // EDGE CASE 2: The Kitchen Sink
        // Testing `*` as a glob buried inside a highly complex spec string
        let complex_str = "conda-forge/linux-64::*[version=\">=2.0\", build=\"*_cpython\"]";

        // We explicitly allow globs for this specific parse
        let options = ParseMatchSpecOptions::from(Strict).with_exact_names_only(false);
        let spec =
            MatchSpec::from_str(complex_str, options).expect("Failed to parse complex glob spec");

        // Verify every single piece was parsed correctly around the `*`
        assert_eq!(
            spec.name,
            PackageNameMatcher::from_str("*").expect("invalid package name matcher")
        );
        assert_eq!(spec.channel.unwrap().name(), "conda-forge");
        assert_eq!(spec.subdir, Some("linux-64".to_string()));
        assert_eq!(
            spec.version,
            Some(VersionSpec::from_str(">=2.0", Strict).expect("invalid version spec"))
        );
        assert!(spec.build.is_some(), "Build string matcher was dropped");
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
        assert!(spec
            .unwrap_err()
            .to_string()
            .contains("multiple values for: build"));

        let spec = MatchSpec::from_str(
            "foo 3.0.* [build=baz, fn='/home/foo.tar.bz2', build='foobar']",
            Strict,
        );
        assert!(spec.is_err());
        assert!(spec
            .unwrap_err()
            .to_string()
            .contains("multiple values for: build"));
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
        let specs = ["mamba 1.0.* py37_0",
            "conda-forge::pytest[version='==1.0', sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97, md5=dede6252c964db3f3e41c7d30d07f6bf]",
            "conda-forge/linux-64::pytest",
            "conda-forge/linux-64::pytest[version=1.0.*]",
            "conda-forge/linux-64::pytest[version=1.0.*, build=py37_0, license=MIT]",
            "conda-forge/linux-64::pytest ==1.2.3"];

        assert_snapshot!(specs
            .into_iter()
            .map(|s| MatchSpec::from_str(s, Strict).unwrap())
            .map(|s| s.to_string())
            .format("\n")
            .to_string());
    }

    #[test]
    fn test_serialize_json_matchspec() {
        let specs = ["mamba 1.0.* py37_0",
            "conda-forge::pytest[version='==1.0', sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97, md5=dede6252c964db3f3e41c7d30d07f6bf]",
            "conda-forge/linux-64::pytest",
            "conda-forge/linux-64::pytest[version=1.0.*]",
            "conda-forge/linux-64::pytest[version=1.0.*, build=py37_0]",
            "conda-forge/linux-64::pytest ==1.2.3"];

        assert_snapshot!(specs
            .into_iter()
            .map(|s| MatchSpec::from_str(s, Strict).unwrap())
            .map(|s| serde_json::to_string(&s).unwrap())
            .format("\n")
            .to_string());
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
