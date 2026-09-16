//! Shared handling for commands that accept a conda package as either a local
//! file path or a remote URL.

use std::path::PathBuf;

use miette::{Context, IntoDiagnostic};
use rattler_package_streaming::archive::PackageArchive;
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

/// A conda package location: either a local file or a remote URL.
#[derive(Debug, Clone)]
pub enum PackageSource {
    Url(Url),
    Path(PathBuf),
}

impl PackageSource {
    /// Parses a command line argument into a local path or a remote URL.
    ///
    /// `file://` URLs are converted to a path. A single letter scheme is
    /// treated as a Windows drive letter (`C:\pkgs\foo.conda`), not a URL.
    pub fn parse(source: &str) -> Self {
        match Url::parse(source) {
            Ok(url) if url.scheme() == "file" => url
                .to_file_path()
                .map_or_else(|()| Self::Path(PathBuf::from(source)), Self::Path),
            Ok(url) if url.scheme().len() > 1 => Self::Url(url),
            _ => Self::Path(PathBuf::from(source)),
        }
    }

    /// Whether this source refers to a remote package.
    pub fn is_url(&self) -> bool {
        matches!(self, Self::Url(_))
    }

    /// Opens the package archive. A `client` must be supplied for remote
    /// sources; see [`client_for`].
    pub async fn open(
        &self,
        client: Option<&ClientWithMiddleware>,
    ) -> miette::Result<PackageArchive> {
        let archive = match self {
            Self::Url(url) => {
                let client = client.expect("a client is created whenever a source is a URL");
                PackageArchive::from_url(client.clone(), url.clone()).await
            }
            Self::Path(path) => PackageArchive::from_path(path).await,
        };
        archive
            .into_diagnostic()
            .with_context(|| format!("failed to open package {self}"))
    }
}

impl std::fmt::Display for PackageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(url) => write!(f, "{url}"),
            Self::Path(path) => write!(f, "{}", path.display()),
        }
    }
}

/// Creates an HTTP client only when at least one of the sources is remote, so
/// purely local invocations never touch the network configuration.
pub fn client_for<'a>(
    sources: impl IntoIterator<Item = &'a PackageSource>,
    offline: bool,
) -> miette::Result<Option<ClientWithMiddleware>> {
    if sources.into_iter().any(PackageSource::is_url) {
        Ok(Some(super::client::create_client_with_middleware(offline)?))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_remote_urls() {
        for source in [
            "https://example.com/pkg-1.0-0.conda",
            "http://example.com/pkg-1.0-0.tar.bz2",
            "s3://bucket/pkg-1.0-0.conda",
        ] {
            assert!(
                matches!(PackageSource::parse(source), PackageSource::Url(_)),
                "{source} should be a URL"
            );
        }
    }

    #[test]
    fn test_parse_local_paths() {
        for source in [
            "/home/user/pkg-1.0-0.conda",
            "./pkg-1.0-0.conda",
            "pkg-1.0-0.tar.bz2",
            // A single letter scheme is a Windows drive letter, not a URL.
            r"C:\packages\pkg-1.0-0.conda",
            "C:/packages/pkg-1.0-0.conda",
        ] {
            assert!(
                matches!(PackageSource::parse(source), PackageSource::Path(_)),
                "{source} should be a path"
            );
        }
    }

    #[test]
    fn test_parse_file_url_becomes_path() {
        let source = PackageSource::parse("file:///home/user/pkg-1.0-0.conda");
        assert!(
            matches!(&source, PackageSource::Path(path) if path.ends_with("pkg-1.0-0.conda")),
            "file:// URL should become a local path, got {source:?}"
        );
    }
}
