use crate::{build_spec::BuildNumberSpec, PackageName, PackageRecord, RepoDataRecord, VersionSpec};
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::{serde_as, skip_serializing_none, DisplayFromStr};
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use std::sync::Arc;
use url::Url;

use crate::Channel;
use crate::ChannelConfig;

pub mod matcher;
pub mod parse;

use matcher::StringMatcher;

/// A [`MatchSpec`] is, fundamentally, a query language for conda packages. Any of the fields that
/// comprise a [`crate::PackageRecord`] can be used to compose a [`MatchSpec`].
///
/// [`MatchSpec`] can be composed with keyword arguments, where keys are any of the
/// attributes of [`crate::PackageRecord`]. Values for keyword arguments are the exact
/// values the attribute should match against. Many fields can also be matched against non-exact
/// values -- by including wildcard `*` and `>`/`<` ranges--where supported. Any non-specified field
/// is the equivalent of a full wildcard match.
///
/// MatchSpecs can also be composed using a single positional argument, with optional
/// keyword arguments. Keyword arguments also override any conflicting information provided in
/// the positional argument. Conda has historically had several string representations for equivalent
/// MatchSpecs.
///
/// A series of rules are now followed for creating the canonical string representation of a
/// MatchSpec instance. The canonical string representation can generically be
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
/// use rattler_conda_types::{MatchSpec, VersionSpec, StringMatcher, PackageName, Channel, ChannelConfig, ParseStrictness::*};
/// use std::str::FromStr;
/// use std::sync::Arc;
///
/// let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
/// let spec = MatchSpec::from_str("foo 1.0 py27_0", Strict).unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str("1.0", Strict).unwrap()));
/// assert_eq!(spec.build, Some(StringMatcher::from_str("py27_0").unwrap()));
///
/// let spec = MatchSpec::from_str("foo 1.0 py27_0", Strict).unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str("==1.0", Strict).unwrap()));
/// assert_eq!(spec.build, Some(StringMatcher::from_str("py27_0").unwrap()));
///
/// let spec = MatchSpec::from_str(r#"conda-forge::foo[version="1.0.*"]"#, Strict).unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str("1.0.*", Strict).unwrap()));
/// assert_eq!(spec.channel, Some(Channel::from_str("conda-forge", &channel_config).map(|channel| Arc::new(channel)).unwrap()));
///
/// let spec = MatchSpec::from_str(r#"conda-forge::foo >=1.0[subdir="linux-64"]"#, Strict).unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str(">=1.0", Strict).unwrap()));
/// assert_eq!(spec.channel, Some(Channel::from_str("conda-forge", &channel_config).map(|channel| Arc::new(channel)).unwrap()));
/// assert_eq!(spec.subdir, Some("linux-64".to_string()));
/// assert_eq!(spec, MatchSpec::from_str("conda-forge/linux-64::foo >=1.0", Strict).unwrap());
///
/// let spec = MatchSpec::from_str("*/linux-64::foo >=1.0", Strict).unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str(">=1.0", Strict).unwrap()));
/// assert_eq!(spec.channel, Some(Channel::from_str("*", &channel_config).map(|channel| Arc::new(channel)).unwrap()));
/// assert_eq!(spec.subdir, Some("linux-64".to_string()));
///
/// let spec = MatchSpec::from_str(r#"foo[build="py2*"]"#, Strict).unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
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
#[derive(Debug, Default, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct MatchSpec {
    /// The name of the package
    pub name: Option<PackageName>,
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    pub version: Option<VersionSpec>,
    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    pub build: Option<StringMatcher>,
    /// The build number of the package
    pub build_number: Option<BuildNumberSpec>,
    /// Match the specific filename of the package
    pub file_name: Option<String>,
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

        match &self.name {
            Some(name) => write!(f, "{}", name.as_normalized())?,
            None => write!(f, "*")?,
        }

        if let Some(version) = &self.version {
            write!(f, " {version}")?;
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

        if let Some(build_number) = &self.build_number {
            keys.push(format!("build_number=\"{build_number}\""));
        }

        if let Some(file_name) = &self.file_name {
            keys.push(format!("fn=\"{file_name}\""));
        }

        if let Some(url) = &self.url {
            keys.push(format!("url=\"{url}\""));
        }

        if !keys.is_empty() {
            write!(f, "[{}]", keys.join(", "))?;
        }

        Ok(())
    }
}

impl MatchSpec {
    /// Decomposes this instance into a [`NamelessMatchSpec`] and a name.
    pub fn into_nameless(self) -> (Option<PackageName>, NamelessMatchSpec) {
        (
            self.name,
            NamelessMatchSpec {
                version: self.version,
                build: self.build,
                build_number: self.build_number,
                file_name: self.file_name,
                channel: self.channel,
                subdir: self.subdir,
                namespace: self.namespace,
                md5: self.md5,
                sha256: self.sha256,
                url: self.url,
            },
        )
    }
}

// Enable constructing a match spec from a package name.
impl From<PackageName> for MatchSpec {
    fn from(value: PackageName) -> Self {
        Self {
            name: Some(value),
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
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub version: Option<VersionSpec>,
    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub build: Option<StringMatcher>,
    /// The build number of the package
    pub build_number: Option<BuildNumberSpec>,
    /// Match the specific filename of the package
    pub file_name: Option<String>,
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
            keys.push(format!("md5={md5:x}"));
        }

        if let Some(sha256) = &self.sha256 {
            keys.push(format!("sha256={sha256:x}"));
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
            channel: spec.channel,
            subdir: spec.subdir,
            namespace: spec.namespace,
            md5: spec.md5,
            sha256: spec.sha256,
            url: spec.url,
        }
    }
}

impl MatchSpec {
    /// Constructs a [`MatchSpec`] from a [`NamelessMatchSpec`] and a name.
    pub fn from_nameless(spec: NamelessMatchSpec, name: Option<PackageName>) -> Self {
        Self {
            name,
            version: spec.version,
            build: spec.build,
            build_number: spec.build_number,
            file_name: spec.file_name,
            channel: spec.channel,
            subdir: spec.subdir,
            namespace: spec.namespace,
            md5: spec.md5,
            sha256: spec.sha256,
            url: spec.url,
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

        true
    }
}

impl Matches<PackageRecord> for MatchSpec {
    /// Match a [`MatchSpec`] against a [`PackageRecord`]
    fn matches(&self, other: &PackageRecord) -> bool {
        if let Some(name) = self.name.as_ref() {
            if name != &other.name {
                return false;
            }
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_digest::{parse_digest_from_hex, Md5, Sha256};

    use crate::{
        match_spec::Matches, MatchSpec, NamelessMatchSpec, PackageName, PackageRecord,
        ParseStrictness::*, RepoDataRecord, StringMatcher, Version,
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
            file_name: String::from("mamba-1.0-py37_0"),
            url: url::Url::parse("https://mamba.io/mamba-1.0-py37_0.conda").unwrap(),
            channel: String::from("mamba"),
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
    fn test_serialize_matchspec() {
        let specs = ["mamba 1.0 py37_0",
            "conda-forge::pytest[version=1.0, sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97, md5=dede6252c964db3f3e41c7d30d07f6bf]",
            "conda-forge/linux-64::pytest",
            "conda-forge/linux-64::pytest[version=1.0]",
            "conda-forge/linux-64::pytest[version=1.0, build=py37_0]",
            "conda-forge/linux-64::pytest 1.2.3"];

        assert_snapshot!(specs
            .into_iter()
            .map(|s| MatchSpec::from_str(s, Strict).unwrap())
            .map(|s| s.to_string())
            .collect::<Vec<String>>()
            .join("\n"));
    }
}
