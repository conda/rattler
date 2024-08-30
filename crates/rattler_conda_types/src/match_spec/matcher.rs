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

/// Error when parsing [`StringMatcher`]
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum StringMatcherParseError {
    /// Could not parse the string as a glob
    #[error("invalid glob: {glob}")]
    InvalidGlob {
        /// The invalid glob
        glob: String,
    },

    /// Could not parse the string as a regex
    #[error("invalid regex: {regex}")]
    InvalidRegex {
        /// The invalid regex
        regex: String,
    },
}

impl FromStr for StringMatcher {
    type Err = StringMatcherParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with('^') && s.ends_with('$') {
            Ok(StringMatcher::Regex(regex::Regex::new(s).map_err(
                |_err| StringMatcherParseError::InvalidRegex {
                    regex: s.to_string(),
                },
            )?))
        } else if s.contains('*') {
            Ok(StringMatcher::Glob(glob::Pattern::new(s).map_err(
                |_err| StringMatcherParseError::InvalidGlob {
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
            StringMatcher::Exact(s) => write!(f, "{s}"),
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
    use assert_matches::assert_matches;

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
    fn test_string_matcher_matches_basic() {
        assert!(StringMatcher::from_str("foo").unwrap().matches("foo"));
        assert!(!StringMatcher::from_str("foo").unwrap().matches("bar"));
    }

    #[test]
    fn test_string_matcher_matches_glob() {
        assert!(StringMatcher::from_str("foo*").unwrap().matches("foobar"));
        assert!(StringMatcher::from_str("*oo").unwrap().matches("foo"));
        assert!(!StringMatcher::from_str("*oo").unwrap().matches("foobar"));
        assert!(StringMatcher::from_str("*oo*").unwrap().matches("foobar"));

        // Conda's glob doesn't care about escaping
        assert!(StringMatcher::from_str("foo\\*bar")
            .unwrap()
            .matches("foo\\bazbar"));
        assert!(!StringMatcher::from_str("foo\\*bar")
            .unwrap()
            .matches("foobazbar"));

        // Or any keywords other than '*'
        assert!(!StringMatcher::from_str("foo[a-z]").unwrap().matches("fooa"));
        assert!(!StringMatcher::from_str("foo[abc]").unwrap().matches("fooa"));
    }

    #[test]
    fn test_string_matcher_matches_regex() {
        assert!(StringMatcher::from_str("^foo.*$")
            .unwrap()
            .matches("foobar"));
        assert!(StringMatcher::from_str("^.*[oo|bar].*$")
            .unwrap()
            .matches("foobar"));
        assert!(!StringMatcher::from_str("^[not].*$")
            .unwrap()
            .matches("foobar"));
        assert!(StringMatcher::from_str("^foo\\[bar\\].*$")
            .unwrap()
            .matches("foo[bar]"));
        assert!(!StringMatcher::from_str("^foo\\[bar\\].*$")
            .unwrap()
            .matches("foobar"));
    }

    #[test]
    fn test_special_characters_matches() {
        let special_characters = "~!@#$%^&*()_-+={}[]|;:'<>,.?/";
        for special_character in special_characters.chars() {
            assert!(StringMatcher::from_str(&special_character.to_string())
                .unwrap()
                .matches(&special_character.to_string()));
        }
    }

    #[test]
    fn test_invalid_regex() {
        let _invalid_regex = "^.*[oo|bar.*$";
        assert_matches!(
            StringMatcher::from_str(_invalid_regex),
            Err(StringMatcherParseError::InvalidRegex {
                regex: _invalid_regex,
            })
        );
    }

    #[test]
    fn test_invalid_glob() {
        let _invalid_glob = "[foo*";
        assert_matches!(
            StringMatcher::from_str(_invalid_glob),
            Err(StringMatcherParseError::InvalidGlob {
                glob: _invalid_glob,
            })
        );
    }

    #[test]
    fn test_empty_strings() {
        assert!(StringMatcher::from_str("").unwrap().matches(""));
        assert!(!StringMatcher::from_str("").unwrap().matches("foo"));

        assert!(!StringMatcher::from_str("foo").unwrap().matches(""));
        assert!(StringMatcher::from_str("^$").unwrap().matches(""));
        assert!(StringMatcher::from_str("*").unwrap().matches(""));
    }
}
