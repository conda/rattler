use file_url::url_to_path;
use itertools::Itertools;
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::hash::Hash;
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
#[derive(Debug, Clone, Eq, SerializeDisplay, DeserializeFromStr)]
pub enum UrlOrPath {
    /// A URL.
    Url(Url),

    /// A local (or networked) path.
    Path(PathBuf),
}

impl PartialOrd<Self> for UrlOrPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for UrlOrPath {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (UrlOrPath::Url(self_url), UrlOrPath::Url(other_url)) => self_url.cmp(other_url),
            (UrlOrPath::Path(self_path), UrlOrPath::Path(other_path)) => self_path.cmp(other_path),
            (UrlOrPath::Url(_), UrlOrPath::Path(_)) => Ordering::Greater,
            (UrlOrPath::Path(_), UrlOrPath::Url(_)) => Ordering::Less,
        }
    }
}

impl PartialEq for UrlOrPath {
    fn eq(&self, other: &Self) -> bool {
        match (self.canonicalize().as_ref(), other.canonicalize().as_ref()) {
            (UrlOrPath::Path(a), UrlOrPath::Path(b)) => a == b,
            (UrlOrPath::Url(a), UrlOrPath::Url(b)) => a == b,
            _ => false,
        }
    }
}

impl Hash for UrlOrPath {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self.canonicalize().as_ref() {
            UrlOrPath::Url(url) => url.hash(state),
            UrlOrPath::Path(path) => path.hash(state),
        }
    }
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
        // Try to normalize the URL to a path if possible.
        if let Some(path) = url_to_path(&value) {
            UrlOrPath::Path(path)
        } else {
            UrlOrPath::Url(value)
        }
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

    /// Canonicalizes the instance to be a path if possible.
    ///
    /// If this instance is a URL with a `file://` scheme, this will try to convert it to a path.
    pub fn canonicalize(&self) -> Cow<'_, Self> {
        if let Some(path) = self.as_url().and_then(url_to_path) {
            return Cow::Owned(UrlOrPath::Path(path));
        }

        Cow::Borrowed(self)
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
        fn scheme_is_drive_letter(scheme: &str) -> bool {
            let Some((drive_letter,)) = scheme.chars().collect_tuple() else {
                return false;
            };
            drive_letter.is_ascii_alphabetic()
        }

        // First try to parse the string as a path.
        return match Url::from_str(s) {
            Ok(url) => Ok(if scheme_is_drive_letter(url.scheme()) {
                UrlOrPath::Path(PathBuf::from(s))
            } else {
                UrlOrPath::Url(url).canonicalize().into_owned()
            }),
            Err(url::ParseError::RelativeUrlWithoutBase) => Ok(UrlOrPath::Path(PathBuf::from(s))),
            Err(e) => Err(PathOrUrlError::InvalidUrl(e)),
        };
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_equality() {
        let tests = [
            // Same urls
            (UrlOrPath::Url("https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2".parse().unwrap()),
             UrlOrPath::Url("https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2".parse().unwrap())),

            // Absolute paths as file and direct path
            (UrlOrPath::Url("file:///home/bob/test-file.txt".parse().unwrap()),
             UrlOrPath::Path("/home/bob/test-file.txt".parse().unwrap())),
        ];

        for (a, b) in &tests {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn test_equality_from_str() {
        let tests = [
            // Same urls
            ("https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2",
             "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2"),

            // Absolute paths as file and direct path
            ("file:///home/bob/test-file.txt", "/home/bob/test-file.txt"),
        ];

        for (a, b) in &tests {
            assert_eq!(
                UrlOrPath::from_str(a).unwrap(),
                UrlOrPath::from_str(b).unwrap()
            );
        }
    }

    #[test]
    fn test_from_str() {
        let urls = [
            "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2",
        ];

        for url in &urls {
            assert_eq!(
                UrlOrPath::from_str(url).unwrap(),
                UrlOrPath::Url(url.parse().unwrap())
            );
        }

        // Test paths
        let paths = [
            // Unix absolute paths
            "/home/bob/test-file.txt",
            // Windows absolute paths
            "c:\\temp\\test-file.txt",
            "c:/temp/test-file.txt",
            // Relative paths
            "./test-file.txt",
            "../test-file.txt",
            // UNC paths
            "\\\\127.0.0.1\\c$\\temp\\test-file.txt",
            "\\\\LOCALHOST\\c$\\temp\\test-file.txt",
            "\\\\.\\c:\\temp\\test-file.txt",
            "\\\\?\\c:\\temp\\test-file.txt",
            "\\\\.\\UNC\\LOCALHOST\\c$\\temp\\test-file.txt",
            "\\\\127.0.0.1\\c$\\temp\\test-file.txt",
        ];

        for path in &paths {
            assert_eq!(
                UrlOrPath::from_str(path).unwrap(),
                UrlOrPath::Path(path.parse().unwrap())
            );
        }
    }
}
