use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    hash::{Hash, Hasher},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A case-insensitive regex matcher that retains the original pattern for
/// display/serialization while compiling the regex with the `(?i)` flag set
/// (per CEP-29, build string matching is always case-insensitive).
#[derive(Debug, Clone)]
pub struct RegexMatcher {
    original: String,
    compiled: fancy_regex::Regex,
}

impl RegexMatcher {
    /// Create a new [`RegexMatcher`] from a regex pattern. The pattern is
    /// compiled with the case-insensitive flag set.
    pub fn new(pattern: &str) -> Result<Self, fancy_regex::Error> {
        let compiled = fancy_regex::Regex::new(&format!("(?i){pattern}"))?;
        Ok(Self {
            original: pattern.to_string(),
            compiled,
        })
    }

    /// Returns the original (non-modified) regex pattern.
    pub fn as_str(&self) -> &str {
        &self.original
    }

    /// Returns true if the regex matches the entire string `other`.
    pub fn is_match(&self, other: &str) -> Result<bool, fancy_regex::Error> {
        self.compiled.is_match(other)
    }
}

/// Match a given string either by exact match, glob or regex.
///
/// Matching is always case-insensitive (ASCII), per CEP-29.
#[derive(Debug, Clone)]
pub enum StringMatcher {
    /// Match the string exactly (case-insensitive, ASCII).
    Exact(String),
    /// Match the string by glob. A glob uses a * to match any characters.
    /// For example, `*` matches any string, `py*` matches any string starting
    /// with `py`, `*37` matches any string ending with `37` and `py*37`
    /// matches any string starting with `py` and ending with `37`.
    /// Matching is case-insensitive.
    Glob(Box<glob::Pattern>),
    /// Match the string by regex. A regex starts with a `^`, ends with a `$`
    /// and uses the regex syntax. For example, `^py.*37$` matches any
    /// string starting with `py` and ending with `37`. Note that the regex
    /// is anchored, so it must match the entire string. Matching is
    /// case-insensitive.
    Regex(Box<RegexMatcher>),
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
    /// Match string against [`StringMatcher`]. Per CEP-29, matching is
    /// always case-insensitive.
    pub fn matches(&self, other: &str) -> bool {
        match self {
            StringMatcher::Exact(s) => s.eq_ignore_ascii_case(other),
            StringMatcher::Glob(glob) => glob.matches_with(
                other,
                glob::MatchOptions {
                    case_sensitive: false,
                    ..glob::MatchOptions::default()
                },
            ),
            // `fancy_regex` can fail on pathological backtracking cases.
            // Treat match errors as non-matches.
            StringMatcher::Regex(regex) => regex.is_match(other).unwrap_or(false),
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
            Ok(StringMatcher::Regex(Box::new(
                RegexMatcher::new(s).map_err(|_err| StringMatcherParseError::InvalidRegex {
                    regex: s.to_string(),
                })?,
            )))
        } else if s.contains('*') {
            Ok(StringMatcher::Glob(Box::new(
                glob::Pattern::new(s).map_err(|_err| StringMatcherParseError::InvalidGlob {
                    glob: s.to_string(),
                })?,
            )))
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
        match self {
            StringMatcher::Exact(s) => s.serialize(serializer),
            StringMatcher::Glob(s) => s.as_str().serialize(serializer),
            StringMatcher::Regex(s) => s.as_str().serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for StringMatcher {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        StringMatcher::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[test]
    fn test_string_matcher() {
        assert_eq!(
            StringMatcher::Exact("foo".to_string()),
            "foo".parse().unwrap()
        );
        assert_eq!(
            StringMatcher::Glob(Box::new(glob::Pattern::new("foo*").unwrap())),
            "foo*".parse().unwrap()
        );
        assert_eq!(
            StringMatcher::Regex(Box::new(RegexMatcher::new("^foo.*$").unwrap())),
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
    fn test_string_matcher_lookahead_regex() {
        // Positive lookahead: match "py" followed by digits, but don't consume the digits
        let matcher = StringMatcher::from_str("^py(?=\\d).*$").unwrap();
        assert!(matcher.matches("py311"));
        assert!(matcher.matches("py39"));
        assert!(!matcher.matches("pypy"));
        assert!(!matcher.matches("python"));

        // Negative lookahead: match "py" NOT followed by "py"
        let matcher = StringMatcher::from_str("^py(?!py).*$").unwrap();
        assert!(matcher.matches("py311"));
        assert!(matcher.matches("python"));
        assert!(!matcher.matches("pypy"));

        // Lookbehind: match strings ending in digits preceded by "py"
        let matcher = StringMatcher::from_str("^.*(?<=py)\\d+$").unwrap();
        assert!(matcher.matches("py311"));
        assert!(matcher.matches("prefix_py39"));
        assert!(!matcher.matches("cp311"));
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
    fn test_case_insensitive_matching() {
        // CEP-29 mandates that build string matching is case-insensitive.

        // Exact
        assert!(StringMatcher::from_str("PyHash_0")
            .unwrap()
            .matches("pyhash_0"));
        assert!(StringMatcher::from_str("pyhash_0")
            .unwrap()
            .matches("PYHASH_0"));

        // Glob
        assert!(StringMatcher::from_str("Py*").unwrap().matches("py37_0"));
        assert!(StringMatcher::from_str("py*").unwrap().matches("Py37_0"));
        assert!(StringMatcher::from_str("*PY39*")
            .unwrap()
            .matches("foo_py39_0"));
        assert!(StringMatcher::from_str("*py39*")
            .unwrap()
            .matches("foo_PY39_0"));

        // Regex
        assert!(StringMatcher::from_str("^Py.*$").unwrap().matches("py37_0"));
        assert!(StringMatcher::from_str("^py.*$").unwrap().matches("PY37_0"));
    }

    #[test]
    fn test_regex_matcher_preserves_original_pattern() {
        // The display / serialization should not leak the (?i) flag we
        // prepend internally.
        let matcher = StringMatcher::from_str("^foo.*$").unwrap();
        assert_eq!(matcher.to_string(), "^foo.*$");
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
