use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

#[derive(Debug, Clone)]
pub enum StringMatcher {
    Exact(String),
    Glob(glob::Pattern),
    Regex(regex::Regex),
}

impl PartialEq for StringMatcher {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (StringMatcher::Exact(s1), StringMatcher::Exact(s2)) => s1 == s2,
            (StringMatcher::Glob(s1), StringMatcher::Glob(s2)) => s1.as_str() == s2.as_str(),
            (StringMatcher::Regex(s1), StringMatcher::Regex(s2)) => s1.as_str() == s2.as_str(),
            _ => false,
        }
    }
}

impl StringMatcher {
    pub(crate) fn matches(&self, other: &str) -> bool {
        match self {
            StringMatcher::Exact(s) => s == other,
            StringMatcher::Glob(s) => s.matches(other),
            StringMatcher::Regex(s) => s.is_match(other),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum StringMatcherParseError {
    #[error("invalid glob: {glob}")]
    InvalidGlob { glob: String },

    #[error("invalid regex: {regex}")]
    InvalidRegex { regex: String },
}

impl FromStr for StringMatcher {
    type Err = StringMatcherParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains('*') {
            Ok(StringMatcher::Glob(glob::Pattern::new(s).map_err(
                |_| StringMatcherParseError::InvalidGlob {
                    glob: s.to_string(),
                },
            )?))
        } else if s.starts_with('^') {
            Ok(StringMatcher::Regex(regex::Regex::new(s).map_err(
                |_| StringMatcherParseError::InvalidRegex {
                    regex: s.to_string(),
                },
            )?))
        } else {
            Ok(StringMatcher::Exact(s.to_string()))
        }
    }
}

impl Display for StringMatcher {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            StringMatcher::Exact(s) => write!(f, "{}", s),
            StringMatcher::Glob(s) => write!(f, "{}", s.as_str()),
            StringMatcher::Regex(s) => write!(f, "{}", s.as_str()),
        }
    }
}

impl Eq for StringMatcher {}

/// implement serde serialization
use serde::{Serialize, Serializer};

impl Serialize for StringMatcher {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_matcher() {
        assert_eq!(
            StringMatcher::Exact("foo".to_string()),
            "foo".parse().unwrap()
        );
        assert_eq!(
            StringMatcher::Glob(glob::Pattern::new("foo*").unwrap()),
            "foo*".parse().unwrap()
        );
        assert_eq!(
            StringMatcher::Regex(regex::Regex::new("^foo.*").unwrap()),
            "^foo.*".parse().unwrap()
        );
    }
}
