use super::{ParseVersionError, Version};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::{
    cmp::Ordering,
    fmt,
    fmt::{Display, Formatter},
    ops::Deref,
    str::FromStr,
};

/// A conda version together with its original textual representation.
///
/// Conda considers `1.0` and `1.00` equal, while this type preserves the input
/// spelling for display and serialization. Converting from [`Version`] uses its
/// canonical display representation instead.
///
/// ```
/// # use rattler_conda_version::version::VersionWithSource;
/// # use std::str::FromStr;
/// let version = VersionWithSource::from_str("1.00").unwrap();
/// assert_eq!(version.to_string(), "1.00");
/// assert_eq!(version.version().to_string(), "1.0");
/// ```
#[derive(Debug, Clone)]
pub struct VersionWithSource {
    version: Version,
    source: Option<Box<str>>,
}

impl FromStr for VersionWithSource {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            version: Version::from_str(s)?,
            source: Some(s.to_owned().into_boxed_str()),
        })
    }
}

impl Hash for VersionWithSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.version.hash(state);
        self.source.hash(state);
    }
}

impl PartialEq for VersionWithSource {
    fn eq(&self, other: &Self) -> bool {
        self.version.eq(&other.version) && self.as_str().eq(&other.as_str())
    }
}

impl Eq for VersionWithSource {}

impl PartialOrd for VersionWithSource {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VersionWithSource {
    fn cmp(&self, other: &Self) -> Ordering {
        // First order by version then by string representation
        self.version
            .cmp(&other.version)
            .then_with(|| self.as_str().cmp(&other.as_str()))
    }
}

impl VersionWithSource {
    /// Associates a parsed [`Version`] with `source` for later display and serialization.
    pub fn new(version: Version, source: impl ToString) -> Self {
        Self {
            version,
            source: Some(source.to_string().into_boxed_str()),
        }
    }

    /// Returns the parsed [`Version`] used for comparison and matching.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// Returns the preserved source text, or the canonical [`Version`] text when none was retained.
    pub fn as_str(&self) -> Cow<'_, str> {
        match &self.source {
            Some(source) => Cow::Borrowed(source.as_ref()),
            None => Cow::Owned(format!("{}", &self.version)),
        }
    }

    /// Consumes this value and returns the parsed [`Version`], discarding its source text.
    pub fn into_version(self) -> Version {
        self.version
    }
}

impl PartialEq<Version> for VersionWithSource {
    fn eq(&self, other: &Version) -> bool {
        self.version.eq(other)
    }
}

impl PartialOrd<Version> for VersionWithSource {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        self.version.partial_cmp(other)
    }
}

impl From<Version> for VersionWithSource {
    fn from(version: Version) -> Self {
        VersionWithSource {
            version,
            source: None,
        }
    }
}

impl From<VersionWithSource> for Version {
    fn from(version: VersionWithSource) -> Self {
        version.version
    }
}

impl AsRef<Version> for VersionWithSource {
    fn as_ref(&self) -> &Version {
        &self.version
    }
}

impl Deref for VersionWithSource {
    type Target = Version;

    fn deref(&self) -> &Self::Target {
        &self.version
    }
}

impl Display for VersionWithSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(source) => write!(f, "{}", source.as_ref()),
            None => write!(f, "{}", &self.version),
        }
    }
}

impl Serialize for VersionWithSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.source {
            None => self.version.to_string().serialize(serializer),
            Some(src) => src.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for VersionWithSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        Ok(Self {
            version: Version::from_str(&source).map_err(D::Error::custom)?,
            source: Some(source.into_boxed_str()),
        })
    }
}
