use serde::{Serialize, Serializer};
use serde_with::DeserializeFromStr;
use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::Arc;

/// A representation of a conda package name. This struct both stores the source string from which
/// this instance was created as well as a normalized name that can be used to compare different
/// names. The normalized name is guaranteed to be a valid conda package name.
///
/// Conda package names are always lowercase.
#[derive(Debug, Clone, Eq, DeserializeFromStr)]
pub struct PackageName {
    source: Arc<str>,
    normalized: Arc<str>,
}

impl PackageName {
    /// Returns the source representation of the package name. This is the string from which this
    /// instance was created.
    pub fn as_source(&self) -> &Arc<str> {
        &self.source
    }

    /// Returns the normalized version of the package name. The normalized string is guaranteed to
    /// be a valid conda package name.
    pub fn as_normalized(&self) -> &Arc<str> {
        &self.normalized
    }
}

impl From<&String> for PackageName {
    fn from(value: &String) -> Self {
        Arc::<str>::from(value.clone()).into()
    }
}

impl From<String> for PackageName {
    fn from(value: String) -> Self {
        Arc::<str>::from(value).into()
    }
}

impl From<Arc<str>> for PackageName {
    fn from(value: Arc<str>) -> Self {
        let normalized = if value.chars().any(char::is_uppercase) {
            Arc::from(value.to_lowercase())
        } else {
            value.clone()
        };
        Self {
            source: value,
            normalized,
        }
    }
}

impl<'a> From<&'a str> for PackageName {
    fn from(value: &'a str) -> Self {
        Arc::<str>::from(value.to_owned()).into()
    }
}

impl FromStr for PackageName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(s))
    }
}

impl Hash for PackageName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.normalized.hash(state)
    }
}

impl PartialEq for PackageName {
    fn eq(&self, other: &Self) -> bool {
        self.normalized.eq(&other.normalized)
    }
}

impl PartialOrd for PackageName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageName {
    fn cmp(&self, other: &Self) -> Ordering {
        self.normalized.cmp(&other.normalized)
    }
}

impl Serialize for PackageName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.source.as_ref().serialize(serializer)
    }
}

impl Display for PackageName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.source.as_ref())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_packagename_basics() {
        let name1 = PackageName::from("cuDNN");
        assert_eq!(name1.as_source().as_ref(), "cuDNN");
        assert_eq!(name1.as_normalized().as_ref(), "cudnn");

        let name2 = PackageName::from("cudnn");
        assert_eq!(name2.as_source().as_ref(), "cudnn");
        assert_eq!(name2.as_normalized().as_ref(), "cudnn");

        assert_eq!(name1, name2);
    }
}
