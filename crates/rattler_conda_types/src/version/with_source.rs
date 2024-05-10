use super::Version;
use crate::ParseVersionError;
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::{
    cmp::Ordering,
    fmt,
    fmt::{Display, Formatter},
    ops::Deref,
    str::FromStr,
};

/// Holds a version and the string it was created from. This is useful if you want to retain the
/// original string the version was created from. This might be useful in cases where you have
/// multiple strings that are represented by the same [`Version`] but you still want to be able to
/// distinguish them.
///
/// The string `1.0` and `1.01` represent the same version. When you print the parsed version though
/// it will come out as `1.0`. You loose the original representation. This struct stores the
/// original source string.
///
/// It is also possible to convert directly from a [`Version`] but the [`Display`] implementation
/// is then used to generate the string representation.
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
    /// Constructs a new instance from a [`Version`] and a source representation.
    pub fn new(version: Version, source: impl ToString) -> Self {
        Self {
            version,
            source: Some(source.to_string().into_boxed_str()),
        }
    }

    /// Returns the [`Version`]
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// Returns the string representation of this instance. Either this is a reference to the source
    /// string or an owned formatted version of the stored version.
    pub fn as_str(&self) -> Cow<'_, str> {
        match &self.source {
            Some(source) => Cow::Borrowed(source.as_ref()),
            None => Cow::Owned(format!("{}", &self.version)),
        }
    }

    /// Convert this instance back into a [`Version`].
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
