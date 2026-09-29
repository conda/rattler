//! Queries: a path, or a pattern with wildcards.
//!
//! * `bin/python`: an exact lookup in the `paths` tables.
//! * `**/zlib.h`, `**/include/zlib.h`, `**/libssl.so*`: a prefix scan of the
//!   `reversed-paths` tables (the literal start of the reversed pattern:
//!   `zlib.h`, `zlib.h/include`, `libssl.so`).
//! * `site-packages/polars/*`: a prefix scan of the `paths` tables (the
//!   literal start of the pattern).

use crate::{Kind, LookupError, format::reverse_components, table::KeyRange};

/// A parsed query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// An exact path.
    Path(String),
    /// A pattern with wildcards.
    Pattern(PathPattern),
}

/// A pattern over paths: `*` and `?` match within a component, `**` any
/// number of components.
///
/// A pattern needs a literal start (of its last component, for `**/`
/// patterns), which becomes the prefix that is scanned; `**/*.h` would read a
/// whole table and is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPattern {
    pattern: String,
    kind: Kind,
    range: KeyRange,
}

impl Query {
    /// Parses a path or pattern. A leading `/` or `./` is dropped: paths are
    /// relative to the environment prefix.
    pub fn parse(query: &str) -> Result<Self, LookupError> {
        let normalized = normalize(query);
        if normalized.is_empty() {
            return Err(LookupError::InvalidQuery {
                query: query.to_string(),
                reason: "the path is empty".into(),
            });
        }
        let wildcard = |s: &str| s.find(['*', '?']);
        if wildcard(normalized).is_none() {
            return Ok(Query::Path(normalized.to_string()));
        }
        let (kind, key) = match normalized.strip_prefix("**/") {
            Some(rest) => (Kind::ReversedPaths, reverse_components(rest)),
            None => (Kind::Paths, normalized.to_string()),
        };
        let prefix = &key[..wildcard(&key).unwrap_or(key.len())];
        if prefix.is_empty() {
            return Err(LookupError::InvalidQuery {
                query: query.to_string(),
                reason: match kind {
                    Kind::ReversedPaths => {
                        "the last component needs a literal start, e.g. `**/libssl.so*` or `**/include/zlib.h`"
                    }
                    Kind::Paths => "it needs a literal start, or has to start with `**/`",
                }
                .into(),
            });
        }
        Ok(Query::Pattern(PathPattern {
            pattern: normalized.to_string(),
            kind,
            range: KeyRange::prefix(prefix),
        }))
    }

    /// The kind of table the query needs.
    pub fn kind(&self) -> Kind {
        match self {
            Query::Path(_) => Kind::Paths,
            Query::Pattern(pattern) => pattern.kind,
        }
    }

    /// The query as given (normalized).
    pub fn as_str(&self) -> &str {
        match self {
            Query::Path(path) => path,
            Query::Pattern(pattern) => &pattern.pattern,
        }
    }
}

impl std::fmt::Display for Query {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Query {
    type Err = LookupError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PathPattern {
    /// The pattern as given.
    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    /// The kind of table the pattern is answered with.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The range of keys that is scanned.
    pub fn key_range(&self) -> &KeyRange {
        &self.range
    }

    /// Whether a key of the table (of the pattern's kind) matches the
    /// pattern.
    pub fn matches_key(&self, key: &str) -> bool {
        self.matches_path(&self.kind.path_of(key))
    }

    /// Whether a path matches the pattern.
    pub fn matches_path(&self, path: &str) -> bool {
        glob_match(&self.pattern, path)
    }
}

/// Whether `path` matches `pattern`: `*` matches within a component, `?` one
/// character except `/`, and a `**` component any number of components.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    if pattern == "**" {
        return true;
    }
    if let Some(rest) = pattern.strip_prefix("**/") {
        // Zero components, or skip one and try again.
        return glob_match(rest, path)
            || path
                .split_once('/')
                .is_some_and(|(_, tail)| glob_match(pattern, tail));
    }
    let mut chars = pattern.chars();
    match chars.next() {
        None => path.is_empty(),
        Some('*') => {
            let rest = chars.as_str();
            let component = path.find('/').unwrap_or(path.len());
            path.char_indices()
                .map(|(i, _)| i)
                .chain([path.len()])
                .take_while(|&i| i <= component)
                .any(|i| glob_match(rest, &path[i..]))
        }
        Some('?') => {
            let mut path_chars = path.chars();
            matches!(path_chars.next(), Some(c) if c != '/')
                && glob_match(chars.as_str(), path_chars.as_str())
        }
        Some(c) => {
            let mut path_chars = path.chars();
            path_chars.next() == Some(c) && glob_match(chars.as_str(), path_chars.as_str())
        }
    }
}

/// Paths are stored relative to the prefix, without a leading `./` or `/`.
fn normalize(path: &str) -> &str {
    let mut path = path;
    loop {
        let trimmed = path.trim_start_matches("./").trim_start_matches('/');
        if trimmed.len() == path.len() {
            return path;
        }
        path = trimmed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_globs() {
        assert!(glob_match("**/zlib.h", "zlib.h"));
        assert!(glob_match("**/zlib.h", "include/zlib.h"));
        assert!(glob_match("**/zlib.h", "Library/include/zlib.h"));
        assert!(!glob_match("**/zlib.h", "include/zlib.hpp"));
        assert!(!glob_match("**/zlib.h", "include/xzlib.h"));
        assert!(glob_match("**/include/zlib.h", "Library/include/zlib.h"));
        assert!(!glob_match("**/include/zlib.h", "zlib.h"));
        assert!(glob_match("**/libssl.so*", "lib/libssl.so.3"));
        assert!(!glob_match("**/libssl.so*", "lib/libssl.so.3/x"));
        assert!(glob_match(
            "site-packages/polars/*",
            "site-packages/polars/__init__.py"
        ));
        assert!(!glob_match(
            "site-packages/polars/*",
            "site-packages/polars/io/a.py"
        ));
        assert!(glob_match(
            "site-packages/polars/**",
            "site-packages/polars/io/a.py"
        ));
        assert!(glob_match("lib/**/libz.so", "lib/libz.so"));
        assert!(glob_match("lib/**/libz.so", "lib/a/b/libz.so"));
        assert!(glob_match("bin/python?", "bin/python3"));
        assert!(!glob_match("bin/python?", "bin/python3.12"));
        assert!(glob_match("bin/p*n*", "bin/python3.12"));
        assert!(glob_match("share/ä?/x", "share/äö/x"));
    }

    #[test]
    fn parses_queries() {
        assert_eq!(
            Query::parse("/bin/python").unwrap(),
            Query::Path("bin/python".into())
        );
        assert_eq!(
            Query::parse("./bin/python").unwrap(),
            Query::Path("bin/python".into())
        );
        let Query::Pattern(pattern) = Query::parse("**/include/zlib.h").unwrap() else {
            panic!("a pattern");
        };
        assert_eq!(pattern.kind(), Kind::ReversedPaths);
        assert_eq!(pattern.key_range(), &KeyRange::prefix("zlib.h/include"));
        assert!(pattern.matches_key("zlib.h/include/Library"));
        assert!(!pattern.matches_key("zlib.h/include2"));

        let Query::Pattern(pattern) = Query::parse("**/libssl.so*").unwrap() else {
            panic!("a pattern");
        };
        assert_eq!(pattern.kind(), Kind::ReversedPaths);
        assert_eq!(pattern.key_range(), &KeyRange::prefix("libssl.so"));

        let Query::Pattern(pattern) = Query::parse("site-packages/polars/*").unwrap() else {
            panic!("a pattern");
        };
        assert_eq!(pattern.kind(), Kind::Paths);
        assert_eq!(
            pattern.key_range(),
            &KeyRange::prefix("site-packages/polars/")
        );
        assert!(pattern.matches_key("site-packages/polars/__init__.py"));

        assert!(matches!(
            Query::parse("**/*.h"),
            Err(LookupError::InvalidQuery { .. })
        ));
        assert!(matches!(
            Query::parse("*/zlib.h"),
            Err(LookupError::InvalidQuery { .. })
        ));
        assert!(matches!(
            Query::parse(""),
            Err(LookupError::InvalidQuery { .. })
        ));
    }
}
