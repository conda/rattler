use std::{
    borrow::Cow,
    cmp::Ordering,
    fmt::{Display, Formatter},
    hash::Hash,
    str::FromStr,
};

use file_url::{file_path_to_url, url_to_typed_path, FileURLParseError};
use itertools::Itertools;
use serde_with::{DeserializeFromStr, SerializeDisplay};
use thiserror::Error;
use typed_path::{Utf8TypedPath, Utf8TypedPathBuf};
use url::Url;

/// Represents either a URL or a path.
///
/// URLs have stricter requirements on their format, they must be absolute and
/// they with the [`url`] we can only create urls for absolute file paths for
/// the current os.
///
/// This also looks better when looking at the lockfile.
#[derive(Debug, Clone, Eq, SerializeDisplay, DeserializeFromStr)]
pub enum UrlOrPath {
    /// A URL.
    Url(Url),

    /// A local (or networked) path.
    Path(Utf8TypedPathBuf),
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
            (UrlOrPath::Path(self_path), UrlOrPath::Path(other_path)) => {
                self_path.as_str().cmp(other_path.as_str())
            }
            (UrlOrPath::Url(_), UrlOrPath::Path(_)) => Ordering::Greater,
            (UrlOrPath::Path(_), UrlOrPath::Url(_)) => Ordering::Less,
        }
    }
}

impl PartialEq for UrlOrPath {
    fn eq(&self, other: &Self) -> bool {
        match (self.normalize().as_ref(), other.normalize().as_ref()) {
            (UrlOrPath::Path(a), UrlOrPath::Path(b)) => a == b,
            (UrlOrPath::Url(a), UrlOrPath::Url(b)) => a == b,
            _ => false,
        }
    }
}

impl Hash for UrlOrPath {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self.normalize().as_ref() {
            UrlOrPath::Url(url) => url.hash(state),
            UrlOrPath::Path(path) => path.as_str().hash(state),
        }
    }
}

impl From<Utf8TypedPathBuf> for UrlOrPath {
    fn from(value: Utf8TypedPathBuf) -> Self {
        UrlOrPath::Path(value)
    }
}

impl<'a> From<Utf8TypedPath<'a>> for UrlOrPath {
    fn from(value: Utf8TypedPath<'a>) -> Self {
        UrlOrPath::Path(value.to_path_buf())
    }
}

impl From<Url> for UrlOrPath {
    fn from(value: Url) -> Self {
        // Try to normalize the URL to a path if possible.
        if let Some(path) = url_to_typed_path(&value) {
            UrlOrPath::Path(path)
        } else {
            UrlOrPath::Url(value)
        }
    }
}

impl Display for UrlOrPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlOrPath::Path(path) => write!(f, "{path}"),
            UrlOrPath::Url(url) => write!(f, "{url}"),
        }
    }
}

impl UrlOrPath {
    /// Returns the string representation of this instance.
    pub fn as_str(&self) -> &str {
        match self {
            UrlOrPath::Url(url) => url.as_str(),
            UrlOrPath::Path(path) => path.as_str(),
        }
    }

    /// Returns the URL if this is a URL.
    pub fn as_url(&self) -> Option<&Url> {
        match self {
            UrlOrPath::Url(url) => Some(url),
            UrlOrPath::Path(_) => None,
        }
    }

    /// Returns the path if this is a path.
    pub fn as_path(&self) -> Option<Utf8TypedPath<'_>> {
        match self {
            UrlOrPath::Path(path) => Some(path.to_path()),
            UrlOrPath::Url(_) => None,
        }
    }

    /// Tries to convert this instance into a URL. This only works if the
    pub fn try_into_url(&self) -> Result<Url, FileURLParseError> {
        match self {
            UrlOrPath::Url(url) => Ok(url.clone()),
            UrlOrPath::Path(path) => file_path_to_url(path.to_path()),
        }
    }

    /// Normalizes the instance to be a path if possible and resolving `..` and
    /// `.` segments.
    ///
    /// If this instance is a URL with a `file://` scheme, this will try to convert it to a path.
    pub fn normalize(&self) -> Cow<'_, Self> {
        match self {
            UrlOrPath::Url(url) => {
                if let Some(path) = url_to_typed_path(url) {
                    return Cow::Owned(UrlOrPath::Path(path.normalize()));
                }
                Cow::Borrowed(self)
            }
            UrlOrPath::Path(path) => Cow::Owned(UrlOrPath::Path(path.normalize())),
        }
    }

    /// Returns the file name of the path or url. If the path or url ends in a
    /// directory seperator `None` is returned.
    pub fn file_name(&self) -> Option<&str> {
        match self {
            UrlOrPath::Path(path) if !path.as_str().ends_with(['/', '\\']) => path.file_name(),
            UrlOrPath::Url(url) if !url.as_str().ends_with('/') => url.path_segments()?.last(),
            _ => None,
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
        fn scheme_is_drive_letter(scheme: &str) -> bool {
            let Some((drive_letter,)) = scheme.chars().collect_tuple() else {
                return false;
            };
            drive_letter.is_ascii_alphabetic()
        }

        // First try to parse the string as a path.
        match Url::from_str(s) {
            Ok(url) => Ok(if scheme_is_drive_letter(url.scheme()) {
                UrlOrPath::Path(s.into())
            } else {
                UrlOrPath::Url(url).normalize().into_owned()
            }),
            Err(url::ParseError::RelativeUrlWithoutBase) => Ok(UrlOrPath::Path(s.into())),
            Err(e) => Err(PathOrUrlError::InvalidUrl(e)),
        }
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use rstest::*;

    use super::*;

    #[rstest]
    #[case(
        "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2",
        Some("_libgcc_mutex-0.1-conda_forge.tar.bz2")
    )]
    #[case(
        "C:\\packages\\_libgcc_mutex-0.1-conda_forge.tar.bz2",
        Some("_libgcc_mutex-0.1-conda_forge.tar.bz2")
    )]
    #[case(
        "/packages/_libgcc_mutex-0.1-conda_forge.tar.bz2",
        Some("_libgcc_mutex-0.1-conda_forge.tar.bz2")
    )]
    #[case("https://conda.anaconda.org/conda-forge/linux-64/", None)]
    #[case("C:\\packages\\", None)]
    #[case("/packages/", None)]
    fn test_file_name(#[case] case: UrlOrPath, #[case] expected_filename: Option<&str>) {
        assert_eq!(case.file_name(), expected_filename);
    }

    #[test]
    fn test_equality() {
        let tests = [
            // Same urls
            (UrlOrPath::Url("https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2".parse().unwrap()),
             UrlOrPath::Url("https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2".parse().unwrap())),

            // Absolute paths as file and direct path
            (UrlOrPath::Url("file:///home/bob/test-file.txt".parse().unwrap()),
             UrlOrPath::Path("/home/bob/test-file.txt".into())),
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

        for path in paths {
            assert_eq!(
                UrlOrPath::from_str(path).unwrap(),
                UrlOrPath::Path(path.into())
            );
        }
    }

    #[test]
    fn test_order() {
        let entries = [
            "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2",
            "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.conda",
            "file:///packages/_libgcc_mutex-0.1-conda_forge.tar.bz2",
            "file:///packages/_libgcc_mutex-0.1-conda_forge.conda",
            "C:\\packages\\_libgcc_mutex-0.1-conda_forge.tar.bz2",
            "/packages/_libgcc_mutex-0.1-conda_forge.tar.bz2",
            "../_libgcc_mutex-0.1-conda_forge.tar.bz2",
            "..\\_libgcc_mutex-0.1-conda_forge.tar.bz2",
        ];

        let sorted_entries = entries
            .iter()
            .map(|p| UrlOrPath::from_str(p).unwrap())
            .sorted()
            .map(|p| p.to_string())
            .format("\n")
            .to_string();
        insta::assert_snapshot!(sorted_entries);
    }
}
