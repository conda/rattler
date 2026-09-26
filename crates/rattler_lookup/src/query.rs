//! What to look up: a path or a glob pattern.
//!
//! Every query is answered by a scan of one lookup table over a range of keys
//! that starts with the literal part of the query. An exact path is a range of
//! one key in the [`Kind::Paths`] table; a pattern starting with `**/` becomes a
//! prefix scan of the [`Kind::ReversedPaths`] table, which is what that table
//! exists for; every other pattern is a prefix scan of the `paths` table.
//!
//! A pattern therefore needs a literal start — `**/include/*.h` is a scan of
//! everything under `h/`-reversed keys, but `**/*.h` would be a scan of the
//! whole index and is rejected.

use crate::{
    LookupError, Result,
    format::{Kind, reverse_components},
};

/// A range of keys, compared bytewise as Parquet statistics are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyRange {
    start: Vec<u8>,
    /// Exclusive end; `None` is unbounded.
    end: Option<Vec<u8>>,
}

impl KeyRange {
    /// Just `key`.
    pub(crate) fn exact(key: &str) -> Self {
        let mut end = key.as_bytes().to_vec();
        end.push(0);
        Self {
            start: key.as_bytes().to_vec(),
            end: Some(end),
        }
    }

    /// All keys starting with `prefix`.
    pub(crate) fn prefix(prefix: &str) -> Self {
        let mut end = prefix.as_bytes().to_vec();
        // The smallest key after all keys with the prefix.
        while end.last() == Some(&0xff) {
            end.pop();
        }
        let end = match end.last_mut() {
            Some(last) => {
                *last += 1;
                Some(end)
            }
            None => None,
        };
        Self {
            start: prefix.as_bytes().to_vec(),
            end,
        }
    }

    /// Whether `[min, max]` — the bounds of a row group or a page, which may be
    /// truncated — may contain a key of the range.
    pub(crate) fn overlaps(&self, min: Option<&[u8]>, max: Option<&[u8]>) -> bool {
        max.is_none_or(|max| max >= self.start.as_slice())
            && match (&self.end, min) {
                (Some(end), Some(min)) => min < end.as_slice(),
                _ => true,
            }
    }

    /// Whether `key` is in the range.
    pub(crate) fn contains(&self, key: &[u8]) -> bool {
        key >= self.start.as_slice() && self.end.as_ref().is_none_or(|end| key < end.as_slice())
    }
}

/// A path or a glob pattern to look up.
///
/// In a pattern, `*` matches within a path component, `?` one character that is
/// not `/`, and a `**` component any number of components.
#[derive(Debug, Clone)]
pub struct Query {
    /// The query, normalized: without a leading `./` or `/`.
    text: String,
    /// Whether [`Self::text`] is a pattern rather than a path.
    is_pattern: bool,
    kind: Kind,
    range: KeyRange,
}

impl Query {
    /// Parses a path or a glob pattern.
    ///
    /// A pattern that does not start with a literal path component or with `**/`
    /// is rejected: answering it would mean scanning the whole index.
    pub fn parse(query: &str) -> Result<Self> {
        let text = normalize(query).to_string();
        let wildcard = |text: &str| text.find(['*', '?']);
        if wildcard(&text).is_none() {
            return Ok(Self {
                range: KeyRange::exact(&text),
                text,
                is_pattern: false,
                kind: Kind::Paths,
            });
        }

        let (kind, key) = match text.strip_prefix("**/") {
            Some(rest) => (Kind::ReversedPaths, reverse_components(rest)),
            None => (Kind::Paths, text.clone()),
        };
        let prefix = &key[..wildcard(&key).unwrap_or(key.len())];
        if prefix.is_empty() {
            return Err(LookupError::InvalidQuery {
                pattern: text,
                reason: match kind {
                    Kind::ReversedPaths => "the last component needs a literal start, \
                         e.g. `**/libssl.so*` or `**/include/zlib.h`"
                        .to_string(),
                    _ => "it needs a literal start or has to begin with `**/`".to_string(),
                },
            });
        }
        Ok(Self {
            range: KeyRange::prefix(prefix),
            text,
            is_pattern: true,
            kind,
        })
    }

    /// The query as it is looked up: without a leading `./` or `/`, because
    /// paths are indexed the way `info/paths.json` spells them.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether this is a pattern, so that several paths can match.
    pub fn is_pattern(&self) -> bool {
        self.is_pattern
    }

    /// The kind of lookup table that answers this query.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Whether `path` is what this query asks for.
    pub fn matches(&self, path: &str) -> bool {
        if self.is_pattern {
            glob_match(&self.text, path)
        } else {
            self.text == path
        }
    }

    /// The range of keys of [`Self::kind`] a scan has to read.
    pub(crate) fn range(&self) -> &KeyRange {
        &self.range
    }
}

/// Whether `path` matches `pattern`: `*` matches within a component, `?` one
/// character except `/`, and a `**` component any number of components.
fn glob_match(pattern: &str, path: &str) -> bool {
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

/// Paths are stored the way `info/paths.json` spells them, i.e. relative to the
/// prefix and with `/` separators.
pub(crate) fn normalize(path: &str) -> &str {
    path.trim_start_matches("./").trim_start_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_ranges() {
        let range = KeyRange::prefix("zlib.h/include");
        assert!(range.contains(b"zlib.h/include"));
        assert!(range.contains(b"zlib.h/include/Library"));
        assert!(!range.contains(b"zlib.h/includf"));
        assert!(!range.contains(b"zlib.h"));
        // Pages whose [min, max] overlaps the range, with truncated bounds.
        assert!(range.overlaps(Some(b"a"), Some(b"zz")));
        assert!(range.overlaps(Some(b"zlib.h/include/x"), Some(b"zz")));
        assert!(!range.overlaps(Some(b"zlib.h/includf"), Some(b"zz")));
        assert!(!range.overlaps(Some(b"a"), Some(b"zlib.h/in")));
        assert!(range.overlaps(None, None));

        // The empty prefix is unbounded in both directions.
        let all = KeyRange::prefix("");
        assert!(all.contains(b""));
        assert!(all.contains(b"anything"));
        assert!(all.overlaps(Some(b"a"), Some(b"b")));
    }

    #[test]
    fn exact_ranges() {
        let range = KeyRange::exact("bin/python");
        assert!(range.contains(b"bin/python"));
        assert!(!range.contains(b"bin/python3"));
        assert!(!range.contains(b"bin/pytho"));
        assert!(range.overlaps(Some(b"bin/python"), Some(b"bin/python")));
        assert!(!range.overlaps(Some(b"bin/python3"), Some(b"bin/z")));
    }

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
        let exact = Query::parse("/bin/python").unwrap();
        assert_eq!(exact.text(), "bin/python");
        assert!(!exact.is_pattern());
        assert_eq!(exact.kind(), Kind::Paths);
        assert_eq!(exact.range(), &KeyRange::exact("bin/python"));
        assert!(exact.matches("bin/python"));
        assert!(!exact.matches("bin/python3"));

        let query = Query::parse("**/include/zlib.h").unwrap();
        assert_eq!(query.kind(), Kind::ReversedPaths);
        assert!(query.is_pattern());
        assert_eq!(query.range(), &KeyRange::prefix("zlib.h/include"));

        let query = Query::parse("**/libssl.so*").unwrap();
        assert_eq!(query.kind(), Kind::ReversedPaths);
        assert_eq!(query.range(), &KeyRange::prefix("libssl.so"));

        let query = Query::parse("site-packages/polars/*").unwrap();
        assert_eq!(query.kind(), Kind::Paths);
        assert_eq!(query.range(), &KeyRange::prefix("site-packages/polars/"));

        // Without a literal start, answering the query would mean reading the
        // whole index.
        for pattern in ["**/*.h", "*/zlib.h", "*"] {
            assert!(
                matches!(Query::parse(pattern), Err(LookupError::InvalidQuery { .. })),
                "{pattern} should be rejected"
            );
        }
    }
}
