//! Where an index file lives: a URL or a local path.

use std::{
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use url::Url;

/// The location of a file of the index: an `http(s)` URL or a local path.
///
/// Layer files are resolved relative to the manifest, so a location knows how
/// to name its siblings.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    /// A remote file, read with HTTP range requests.
    Url(Url),
    /// A local file.
    Path(PathBuf),
}

impl Location {
    /// Interprets a string as a URL (`http`, `https` or `file`) or as a local
    /// path.
    pub fn parse(location: &str) -> Self {
        match Url::parse(location) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => Self::Url(url),
            Ok(url) if url.scheme() == "file" => match url.to_file_path() {
                Ok(path) => Self::Path(path),
                Err(()) => Self::Path(PathBuf::from(location)),
            },
            _ => Self::Path(PathBuf::from(location)),
        }
    }

    /// The location of `name` in the same directory as this one.
    pub fn sibling(&self, name: &str) -> Self {
        match self {
            Self::Url(url) => match url.join(name) {
                Ok(joined) => Self::Url(joined),
                // A relative reference that does not resolve against the URL
                // (a `cannot-be-a-base` URL): fall back to string concatenation.
                Err(_) => Self::Url(url.clone()),
            },
            Self::Path(path) => Self::Path(path.with_file_name(name)),
        }
    }

    /// The location of `relative` (a `/`-separated relative path) below this
    /// one, which is treated as a directory.
    pub fn join(&self, relative: &str) -> Self {
        match self {
            Self::Url(url) => {
                let mut base = url.clone();
                if !base.path().ends_with('/') {
                    base.set_path(&format!("{}/", base.path()));
                }
                match base.join(relative) {
                    Ok(joined) => Self::Url(joined),
                    Err(_) => Self::Url(base),
                }
            }
            Self::Path(path) => {
                let mut path = path.clone();
                for component in relative.split('/').filter(|c| !c.is_empty()) {
                    path.push(component);
                }
                Self::Path(path)
            }
        }
    }

    /// Resolves a `lookup_url` from the repodata (an absolute URL, or a URL
    /// relative to the repodata file at `self`) to a location.
    pub fn resolve_lookup_url(&self, lookup_url: &str) -> Self {
        if let Ok(url) = Url::parse(lookup_url)
            && matches!(url.scheme(), "http" | "https" | "file")
        {
            return Self::parse(lookup_url);
        }
        self.sibling(lookup_url)
    }

    /// The URL of a remote location.
    pub fn as_url(&self) -> Option<&Url> {
        match self {
            Self::Url(url) => Some(url),
            Self::Path(_) => None,
        }
    }

    /// The path of a local location.
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Self::Url(_) => None,
            Self::Path(path) => Some(path),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url(url) => url.fmt(f),
            Self::Path(path) => path.display().fmt(f),
        }
    }
}

impl FromStr for Location {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

impl From<Url> for Location {
    fn from(url: Url) -> Self {
        Self::parse(url.as_str())
    }
}

impl From<PathBuf> for Location {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<&Path> for Location {
    fn from(path: &Path) -> Self {
        Self::Path(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls_and_paths() {
        assert!(matches!(
            Location::parse("https://example.org/c/noarch/lookup/manifest.json"),
            Location::Url(_)
        ));
        assert!(matches!(Location::parse("idx/noarch"), Location::Path(_)));
        assert!(matches!(Location::parse("C:\\idx"), Location::Path(_)));
        assert_eq!(
            Location::parse("file:///tmp/idx"),
            Location::Path(PathBuf::from("/tmp/idx"))
        );
    }

    #[test]
    fn resolves_siblings_and_children() {
        let manifest = Location::parse("https://x.org/c/noarch/lookup/manifest.json");
        assert_eq!(
            manifest.sibling("a.parquet").to_string(),
            "https://x.org/c/noarch/lookup/a.parquet"
        );
        let base = Location::parse("https://x.org/c");
        assert_eq!(
            base.join("noarch/lookup/manifest.json").to_string(),
            "https://x.org/c/noarch/lookup/manifest.json"
        );
        let base = Location::parse("idx");
        assert_eq!(
            base.join("noarch/lookup/manifest.json"),
            Location::Path(PathBuf::from("idx/noarch/lookup/manifest.json"))
        );
        assert_eq!(
            Location::parse("idx/noarch/lookup/manifest.json").sibling("a.parquet"),
            Location::Path(PathBuf::from("idx/noarch/lookup/a.parquet"))
        );
    }

    #[test]
    fn resolves_lookup_urls() {
        let repodata = Location::parse("https://x.org/c/noarch/repodata.json");
        assert_eq!(
            repodata
                .resolve_lookup_url("./lookup/manifest.json")
                .to_string(),
            "https://x.org/c/noarch/lookup/manifest.json"
        );
        assert_eq!(
            repodata
                .resolve_lookup_url("https://y.org/m.json")
                .to_string(),
            "https://y.org/m.json"
        );
        let repodata = Location::parse("channel/noarch/repodata.json");
        assert_eq!(
            repodata.resolve_lookup_url("./lookup/manifest.json"),
            Location::Path(PathBuf::from("channel/noarch/./lookup/manifest.json"))
        );
    }
}
