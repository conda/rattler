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
pub enum PathOrUrl {
    /// A URL.
    Url(Url),

    /// A local (or networked) path.
    Path(PathBuf),
}

impl From<PathBuf> for PathOrUrl {
    fn from(value: PathBuf) -> Self {
        PathOrUrl::Path(value)
    }
}

impl From<&Path> for PathOrUrl {
    fn from(value: &Path) -> Self {
        PathOrUrl::Path(value.to_path_buf())
    }
}

impl From<Url> for PathOrUrl {
    fn from(value: Url) -> Self {
        PathOrUrl::Url(value)
    }
}

impl Display for PathOrUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PathOrUrl::Path(path) => write!(f, "{}", path.display()),
            PathOrUrl::Url(url) => write!(f, "{url}"),
        }
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum PathOrUrlError {
    #[error(transparent)]
    InvalidUrl(url::ParseError),
}

impl FromStr for PathOrUrl {
    type Err = PathOrUrlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Url::from_str(s) {
            Ok(url) => Ok(PathOrUrl::Url(url)),
            Err(url::ParseError::RelativeUrlWithoutBase) => Ok(PathOrUrl::Path(PathBuf::from(s))),
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
            PathOrUrl::from_str("https://example.com").unwrap(),
            PathOrUrl::Url("https://example.com".parse().unwrap())
        );
        assert_eq!(
            PathOrUrl::from_str("file:///path/to/file").unwrap(),
            PathOrUrl::Url("file:///path/to/file".parse().unwrap())
        );
        assert_eq!(
            PathOrUrl::from_str("/path/to/file").unwrap(),
            PathOrUrl::Path("/path/to/file".parse().unwrap()),
        );
        assert_eq!(
            PathOrUrl::from_str("./path/to/file").unwrap(),
            PathOrUrl::Path("./path/to/file".parse().unwrap())
        );
    }
}
