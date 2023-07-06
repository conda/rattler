use super::{parse::ParseMatchSpecError, MatchSpec};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    borrow::Cow,
    fmt,
    fmt::{Display, Formatter},
    ops::Deref,
    str::FromStr,
};

/// Holds both a [`MatchSpec`] and the source string it was parsed from. This enables recreating the
/// exact string that was parsed which is useful if you for instance want to recreate original conda
/// datastructures.
#[derive(Debug, Clone)]
pub struct MatchSpecWithSource {
    match_spec: MatchSpec,
    source: Option<Box<str>>,
}

impl FromStr for MatchSpecWithSource {
    type Err = ParseMatchSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            match_spec: MatchSpec::from_str(s)?,
            source: Some(s.to_owned().into_boxed_str()),
        })
    }
}

impl PartialEq for MatchSpecWithSource {
    fn eq(&self, other: &Self) -> bool {
        self.match_spec.eq(&other.match_spec) && self.as_str().eq(&other.as_str())
    }
}

impl Eq for MatchSpecWithSource {}

impl MatchSpecWithSource {
    /// Constructs a new instance from a [`MatchSpec`] and a source representation.
    pub fn new(match_spec: MatchSpec, source: impl ToString) -> Self {
        Self {
            match_spec,
            source: Some(source.to_string().into_boxed_str()),
        }
    }

    /// Returns the [`MatchSpec`]
    pub fn match_spec(&self) -> &MatchSpec {
        &self.match_spec
    }

    /// Returns the string representation of this instance. Either this is a reference to the source
    /// string or an owned formatted spec of the stored [`MatchSpec`].
    pub fn as_str(&self) -> Cow<str> {
        match &self.source {
            Some(source) => Cow::Borrowed(source.as_ref()),
            None => Cow::Owned(format!("{}", &self.match_spec)),
        }
    }

    /// Convert this instance back into a [`MatchSpec`].
    pub fn into_match_spec(self) -> MatchSpec {
        self.match_spec
    }
}

impl From<MatchSpec> for MatchSpecWithSource {
    fn from(match_spec: MatchSpec) -> Self {
        MatchSpecWithSource {
            match_spec,
            source: None,
        }
    }
}

impl From<MatchSpecWithSource> for MatchSpec {
    fn from(spec: MatchSpecWithSource) -> Self {
        spec.match_spec
    }
}

impl AsRef<MatchSpec> for MatchSpecWithSource {
    fn as_ref(&self) -> &MatchSpec {
        &self.match_spec
    }
}

impl Deref for MatchSpecWithSource {
    type Target = MatchSpec;

    fn deref(&self) -> &Self::Target {
        &self.match_spec
    }
}

impl Display for MatchSpecWithSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(source) => write!(f, "{}", source.as_ref()),
            None => write!(f, "{}", &self.match_spec),
        }
    }
}

impl Serialize for MatchSpecWithSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.source {
            None => self.match_spec.to_string().serialize(serializer),
            Some(src) => src.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MatchSpecWithSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        Ok(Self {
            match_spec: MatchSpec::from_str(&source).map_err(D::Error::custom)?,
            source: Some(source.into_boxed_str()),
        })
    }
}
