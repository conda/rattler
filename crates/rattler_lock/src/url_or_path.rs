use itertools::Itertools;
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

fn scheme_is_drive_letter(scheme: &str) -> bool {
    let Some((drive_letter,)) = scheme.chars().collect_tuple() else {
        return false;
    };
    drive_letter.is_ascii_alphabetic()
}

impl FromStr for UrlOrPath {
    type Err = PathOrUrlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First try to parse the string as a path.
        match Url::from_str(s) {
            Ok(url) => Ok(if scheme_is_drive_letter(url.scheme()) {
                UrlOrPath::Path(PathBuf::from(s))
            } else {
                UrlOrPath::Url(url)
            }),
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
