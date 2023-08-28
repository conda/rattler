use serde::{Serialize, Serializer};
use std::hash::{Hash, Hasher};
use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

/// Match a given string either by exact match, glob or regex
#[derive(Debug, Clone)]
pub enum StringMatcher {
    /// Match the string exactly
    Exact(String),
    /// Match the string by glob. A glob uses a * to match any characters.
    /// For example, `*` matches any string, `py*` matches any string starting with `py`,
    /// `*37` matches any string ending with `37` and `py*37` matches any string starting with `py` and ending with `37`.
    Glob(glob::Pattern),
    /// Match the string by regex. A regex starts with a `^`, ends with a `$` and uses the regex syntax.
    /// For example, `^py.*37$` matches any string starting with `py` and ending with `37`.
    /// Note that the regex is anchored, so it must match the entire string.
    Regex(regex::Regex),
}

impl Hash for StringMatcher {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            StringMatcher::Exact(s) => s.hash(state),
            StringMatcher::Glob(pattern) => pattern.hash(state),
            StringMatcher::Regex(regex) => regex.as_str().hash(state),
        }
    }
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
    /// Match string against [`StringMatcher`].
    pub fn matches(&self, other: &str) -> bool {
        match self {
            StringMatcher::Exact(s) => s == other,
            StringMatcher::Glob(glob) => glob.matches(other),
            StringMatcher::Regex(regex) => regex.is_match(other),
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
        if s.starts_with('^') && s.ends_with('$') {
            Ok(StringMatcher::Regex(regex::Regex::new(s).map_err(
                |_| StringMatcherParseError::InvalidRegex {
                    regex: s.to_string(),
                },
            )?))
        } else if s.contains('*') {
            Ok(StringMatcher::Glob(glob::Pattern::new(s).map_err(
                |_| StringMatcherParseError::InvalidGlob {
                    glob: s.to_string(),
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
            StringMatcher::Regex(regex::Regex::new("^foo.*$").unwrap()),
            "^foo.*$".parse().unwrap()
        );
    }

    #[test]
    fn test_string_matcher_matches() {
        assert!(StringMatcher::from_str("foo").unwrap().matches("foo"));
        assert!(!StringMatcher::from_str("foo").unwrap().matches("bar"));
        assert!(StringMatcher::from_str("foo*").unwrap().matches("foobar"));
        assert!(StringMatcher::from_str("*oo").unwrap().matches("foo"));
        assert!(!StringMatcher::from_str("*oo").unwrap().matches("foobar"));
        assert!(StringMatcher::from_str("*oo*").unwrap().matches("foobar"));
        assert!(StringMatcher::from_str("^foo.*$")
            .unwrap()
            .matches("foobar"));
        assert!(StringMatcher::from_str("^.*[oo|bar].*$")
            .unwrap()
            .matches("foobar"));
        assert!(!StringMatcher::from_str("^[not].*$")
            .unwrap()
            .matches("foobar"));
    }
}
