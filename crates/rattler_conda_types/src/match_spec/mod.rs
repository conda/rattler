use crate::{build_spec::BuildNumberSpec, PackageName, PackageRecord, VersionSpec};
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, DisplayFromStr};
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;

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
/// use rattler_conda_types::{MatchSpec, VersionSpec, StringMatcher, PackageName};
/// use std::str::FromStr;
///
/// let spec = MatchSpec::from_str("foo 1.0 py27_0").unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str("1.0").unwrap()));
/// assert_eq!(spec.build, Some(StringMatcher::from_str("py27_0").unwrap()));
///
/// let spec = MatchSpec::from_str("foo=1.0=py27_0").unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str("==1.0").unwrap()));
/// assert_eq!(spec.build, Some(StringMatcher::from_str("py27_0").unwrap()));
///
/// let spec = MatchSpec::from_str("conda-forge::foo[version=\"1.0.*\"]").unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str("1.0.*").unwrap()));
/// assert_eq!(spec.channel, Some("conda-forge".to_string()));
///
/// let spec = MatchSpec::from_str("conda-forge/linux-64::foo>=1.0").unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str(">=1.0").unwrap()));
/// assert_eq!(spec.channel, Some("conda-forge".to_string()));
/// assert_eq!(spec.subdir, Some("linux-64".to_string()));
///
/// let spec = MatchSpec::from_str("*/linux-64::foo>=1.0").unwrap();
/// assert_eq!(spec.name, Some(PackageName::new_unchecked("foo")));
/// assert_eq!(spec.version, Some(VersionSpec::from_str(">=1.0").unwrap()));
/// assert_eq!(spec.channel, Some("*".to_string()));
/// assert_eq!(spec.subdir, Some("linux-64".to_string()));
///
/// let spec = MatchSpec::from_str("foo[build=\"py2*\"]").unwrap();
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
    pub channel: Option<String>,
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
}

impl Display for MatchSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(channel) = &self.channel {
            // TODO: namespace
            write!(f, "{}", channel)?;
        }

        if let Some(subdir) = &self.subdir {
            write!(f, "/{}", subdir)?;
        }

        match &self.name {
            Some(name) => write!(f, "{}", name.as_normalized())?,
            None => write!(f, "*")?,
        }

        if let Some(namespace) = &self.namespace {
            write!(f, ":{}:", namespace)?;
        } else if self.channel.is_some() || self.subdir.is_some() {
            write!(f, "::")?;
        }

        if let Some(version) = &self.version {
            write!(f, " {}", version)?;
        }

        if let Some(build) = &self.build {
            write!(f, " {}", build)?;
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

impl MatchSpec {
    /// Match a MatchSpec against a PackageRecord
    pub fn matches(&self, record: &PackageRecord) -> bool {
        if let Some(name) = self.name.as_ref() {
            if name != &record.name {
                return false;
            }
        }

        if let Some(spec) = self.version.as_ref() {
            if !spec.matches(&record.version) {
                return false;
            }
        }

        if let Some(build_string) = self.build.as_ref() {
            if !build_string.matches(&record.build) {
                return false;
            }
        }

        if let Some(build_number) = self.build_number.as_ref() {
            if !build_number.matches(&record.build_number) {
                return false;
            }
        }

        if let Some(md5_spec) = self.md5.as_ref() {
            if Some(md5_spec) != record.md5.as_ref() {
                return false;
            }
        }

        if let Some(sha256_spec) = self.sha256.as_ref() {
            if Some(sha256_spec) != record.sha256.as_ref() {
                return false;
            }
        }

        true
    }

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
            },
        )
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
    pub channel: Option<String>,
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
}

impl NamelessMatchSpec {
    /// Match a MatchSpec against a PackageRecord
    pub fn matches(&self, record: &PackageRecord) -> bool {
        if let Some(spec) = self.version.as_ref() {
            if !spec.matches(&record.version) {
                return false;
            }
        }

        if let Some(build_string) = self.build.as_ref() {
            if !build_string.matches(&record.build) {
                return false;
            }
        }

        if let Some(md5_spec) = self.md5.as_ref() {
            if Some(md5_spec) != record.md5.as_ref() {
                return false;
            }
        }

        if let Some(sha256_spec) = self.sha256.as_ref() {
            if Some(sha256_spec) != record.sha256.as_ref() {
                return false;
            }
        }

        true
    }
}

impl Display for NamelessMatchSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.version {
            Some(version) => write!(f, "{version}")?,
            None => write!(f, "*")?,
        }

        if let Some(build) = &self.build {
            write!(f, " {}", build)?;
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
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_digest::{parse_digest_from_hex, Md5, Sha256};

    use crate::{MatchSpec, NamelessMatchSpec, PackageName, PackageRecord, Version};
    use std::hash::{Hash, Hasher};

    #[test]
    fn test_matchspec_format_eq() {
        let spec = MatchSpec::from_str("mamba[version==1.0, sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97, md5=dede6252c964db3f3e41c7d30d07f6bf]").unwrap();
        let spec_as_string = spec.to_string();
        let rebuild_spec = MatchSpec::from_str(&spec_as_string).unwrap();

        assert_eq!(spec, rebuild_spec)
    }

    #[test]
    fn test_nameless_matchspec_format_eq() {
        let spec = NamelessMatchSpec::from_str("*[version==1.0, sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97, md5=dede6252c964db3f3e41c7d30d07f6bf]").unwrap();
        let spec_as_string = spec.to_string();
        let rebuild_spec = NamelessMatchSpec::from_str(&spec_as_string).unwrap();

        assert_eq!(spec, rebuild_spec)
    }

    #[test]
    fn test_hash_match() {
        let spec1 = MatchSpec::from_str("tensorflow 2.6.*").unwrap();
        let spec2 = MatchSpec::from_str("tensorflow 2.6.*").unwrap();
        assert_eq!(spec1, spec2);

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let hash1 = spec1.hash(&mut hasher);
        let hash2 = spec2.hash(&mut hasher);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_no_match() {
        let spec1 = MatchSpec::from_str("tensorflow 2.6.0.*").unwrap();
        let spec2 = MatchSpec::from_str("tensorflow 2.6.*").unwrap();
        assert_ne!(spec1, spec2);

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        spec1.hash(&mut hasher);
        let hash1 = hasher.finish();
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
                String::from(""),
            )
        };

        let spec = MatchSpec::from_str("mamba[version==1.0, sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97]").unwrap();
        assert!(!spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[version==1.0, sha256=f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97]").unwrap();
        assert!(spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[version==1.0, md5=aaaa6252c964db3f3e41c7d30d07f6bf]")
            .unwrap();
        assert!(!spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[version==1.0, md5=dede6252c964db3f3e41c7d30d07f6bf]")
            .unwrap();
        assert!(spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[version==1.0, md5=dede6252c964db3f3e41c7d30d07f6bf, sha256=f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97]").unwrap();
        assert!(spec.matches(&record));

        let spec = MatchSpec::from_str("mamba[version==1.0, md5=dede6252c964db3f3e41c7d30d07f6bf, sha256=aaac4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97]").unwrap();
        assert!(!spec.matches(&record));
    }
}
