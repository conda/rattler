use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::path::Path;
use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
    str::FromStr,
};
use thiserror::Error;
use url::Url;

/// Represents either a URL or a path.
///
/// URLs have stricter requirements on their format, they must be absolute and they with the
/// [`url`] we can only create urls for absolute file paths for the current os.
///
/// This also looks better when looking at the lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Hash, SerializeDisplay, DeserializeFromStr)]
pub enum UrlOrPath {
    /// A URL.
    Url(Url),

    /// A local (or networked) path.
    Path(PathBuf),
}

impl From<PathBuf> for UrlOrPath {
    fn from(value: PathBuf) -> Self {
        UrlOrPath::Path(value)
    }
}

impl From<&Path> for UrlOrPath {
    fn from(value: &Path) -> Self {
        UrlOrPath::Path(value.to_path_buf())
    }
}

impl From<Url> for UrlOrPath {
    fn from(value: Url) -> Self {
        UrlOrPath::Url(value)
    }
}

impl Display for UrlOrPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlOrPath::Path(path) => write!(f, "{}", path.display()),
            UrlOrPath::Url(url) => write!(f, "{url}"),
        }
    }
}

impl UrlOrPath {
    /// Returns the URL if this is a URL.
    pub fn as_url(&self) -> Option<&Url> {
        match self {
            UrlOrPath::Url(url) => Some(url),
            UrlOrPath::Path(_) => None,
        }
    }

    /// Returns the path if this is a path.
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            UrlOrPath::Path(path) => Some(path),
            UrlOrPath::Url(_) => None,
        }
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum PathOrUrlError {
    #[error(transparent)]
    InvalidUrl(url::ParseError),
}

impl FromStr for UrlOrPath {
    type Err = PathOrUrlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Url::from_str(s) {
            Ok(url) => Ok(UrlOrPath::Url(url)),
            Err(url::ParseError::RelativeUrlWithoutBase) => Ok(UrlOrPath::Path(PathBuf::from(s))),
            Err(e) => Err(PathOrUrlError::InvalidUrl(e)),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_from_str() {
        assert_eq!(
            UrlOrPath::from_str("https://example.com").unwrap(),
            UrlOrPath::Url("https://example.com".parse().unwrap())
        );
        assert_eq!(
            UrlOrPath::from_str("file:///path/to/file").unwrap(),
            UrlOrPath::Url("file:///path/to/file".parse().unwrap())
        );
        assert_eq!(
            UrlOrPath::from_str("/path/to/file").unwrap(),
            UrlOrPath::Path("/path/to/file".parse().unwrap()),
        );
        assert_eq!(
            UrlOrPath::from_str("./path/to/file").unwrap(),
            UrlOrPath::Path("./path/to/file".parse().unwrap())
        );
    }
}
