use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    hash::{Hash, Hasher},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{InvalidPackageNameError, PackageName};

/// Match a given string either by exact match, glob or regex
#[derive(Debug, Clone)]
pub enum PackageNameMatcher {
    /// Match the string exactly
    Exact(PackageName),
    /// Match the string by glob. A glob uses a * to match any characters.
    /// For example, `*` matches any string, `foo*` matches any string starting
    /// with `foo`, `*bar` matches any string ending with `bar` and `foo*bar`
    /// matches any string starting with `foo` and ending with `bar`.
    Glob(glob::Pattern),
    /// Match the string by regex. A regex starts with a `^`, ends with a `$`
    /// and uses the regex syntax. For example, `^foo.*bar$` matches any
    /// string starting with `foo` and ending with `bar`. Note that the regex
    /// is anchored, so it must match the entire string.
    Regex(regex::Regex),
}

impl Hash for PackageNameMatcher {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            PackageNameMatcher::Exact(s) => s.hash(state),
            PackageNameMatcher::Glob(pattern) => pattern.hash(state),
            PackageNameMatcher::Regex(regex) => regex.as_str().hash(state),
        }
    }
}

impl PartialEq for PackageNameMatcher {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PackageNameMatcher::Exact(s1), PackageNameMatcher::Exact(s2)) => s1 == s2,
            (PackageNameMatcher::Glob(s1), PackageNameMatcher::Glob(s2)) => {
                s1.as_str() == s2.as_str()
            }
            (PackageNameMatcher::Regex(s1), PackageNameMatcher::Regex(s2)) => {
                s1.as_str() == s2.as_str()
            }
            _ => false,
        }
    }
}

impl PackageNameMatcher {
    /// Match string against [`PackageNameMatcher`].
    pub fn matches(&self, other: &PackageName) -> bool {
        match self {
            PackageNameMatcher::Exact(s) => s == other,
            PackageNameMatcher::Glob(glob) => glob.matches(other.as_normalized()),
            PackageNameMatcher::Regex(regex) => regex.is_match(other.as_normalized()),
        }
    }

    /// Convert [`PackageNameMatcher`] to [`PackageName`].
    pub fn into_exact(self) -> PackageName {
        match self {
            PackageNameMatcher::Exact(s) => s,
            _ => panic!("PackageNameMatcher is not Exact"),
        }
    }

    /// Convert [`PackageNameMatcher`] to [`PackageName`].
    pub fn as_exact(&self) -> &PackageName {
        match self {
            PackageNameMatcher::Exact(s) => s,
            _ => panic!("PackageNameMatcher is not Exact"),
        }
    }
}

/// Error when parsing [`PackageNameMatcher`]
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum PackageNameMatcherParseError {
    /// Could not parse the string as a glob
    #[error("invalid glob: {glob}")]
    Glob {
        /// The invalid glob
        glob: String,
    },

    /// Could not parse the string as a regex
    #[error("invalid regex: {regex}")]
    Regex {
        /// The invalid regex
        regex: String,
    },

    /// Could not parse the string as a package name
    #[error("invalid package name {name}: {source}")]
    PackageName {
        /// The invalid package name
        name: String,

        /// The source error
        source: InvalidPackageNameError,
    },
}

impl FromStr for PackageNameMatcher {
    type Err = PackageNameMatcherParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with('^') && s.ends_with('$') {
            Ok(PackageNameMatcher::Regex(regex::Regex::new(s).map_err(
                |_err| PackageNameMatcherParseError::Regex {
                    regex: s.to_string(),
                },
            )?))
        } else if s.contains('*') {
            Ok(PackageNameMatcher::Glob(glob::Pattern::new(s).map_err(
                |_err| PackageNameMatcherParseError::Glob {
                    glob: s.to_string(),
                },
            )?))
        } else {
            Ok(PackageNameMatcher::Exact(
                PackageName::from_str(s).map_err(|e| {
                    PackageNameMatcherParseError::PackageName {
                        name: s.to_string(),
                        source: e,
                    }
                })?,
            ))
        }
    }
}

impl Display for PackageNameMatcher {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageNameMatcher::Exact(s) => write!(f, "{}", s.as_normalized()),
            PackageNameMatcher::Glob(s) => write!(f, "{}", s.as_str()),
            PackageNameMatcher::Regex(s) => write!(f, "{}", s.as_str()),
        }
    }
}

impl Eq for PackageNameMatcher {}

impl Serialize for PackageNameMatcher {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            PackageNameMatcher::Exact(s) => s.serialize(serializer),
            PackageNameMatcher::Glob(s) => s.as_str().serialize(serializer),
            PackageNameMatcher::Regex(s) => s.as_str().serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for PackageNameMatcher {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        PackageNameMatcher::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Error when converting a [`PackageNameMatcher`] to a [`PackageName`]
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum IntoPackageNameError {
    /// The package name matcher is not an exact package name
    #[error("not an exact package name")]
    NotExact,
}

impl TryInto<PackageName> for PackageNameMatcher {
    type Error = IntoPackageNameError;
    fn try_into(self) -> Result<PackageName, Self::Error> {
        match self {
            PackageNameMatcher::Exact(name) => Ok(name),
            _ => Err(IntoPackageNameError::NotExact),
        }
    }
}

impl TryInto<PackageName> for &PackageNameMatcher {
    type Error = IntoPackageNameError;
    fn try_into(self) -> Result<PackageName, Self::Error> {
        match self {
            PackageNameMatcher::Exact(name) => Ok(name.clone()),
            _ => Err(IntoPackageNameError::NotExact),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_name_matcher() {
        assert_eq!(
            PackageNameMatcher::Exact(PackageName::from_str("foo").unwrap()),
            "foo".parse().unwrap()
        );
        assert_eq!(
            PackageNameMatcher::Glob(glob::Pattern::new("foo*").unwrap()),
            "foo*bar".parse().unwrap()
        );
        assert_eq!(
            PackageNameMatcher::Regex(regex::Regex::new("^foo.*$").unwrap()),
            "^foo.*$".parse().unwrap()
        );
    }
}
