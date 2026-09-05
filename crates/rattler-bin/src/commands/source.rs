//! Shared handling of package arguments that can be either a local path or a
//! remote URL.

use std::path::PathBuf;

use miette::{Context, IntoDiagnostic};
use rattler_package_streaming::archive::PackageArchive;
use url::Url;

/// A package location: either a local file or a remote URL.
pub enum PackageSource {
    Url(Url),
    Path(PathBuf),
}

impl PackageSource {
    /// Parses a command line argument into a local path or a remote URL. A
    /// single letter scheme is treated as a Windows drive letter.
    pub fn parse(source: &str) -> Self {
        match Url::parse(source) {
            Ok(url) if url.scheme() == "file" => url.to_file_path().map_or_else(
                |()| PackageSource::Path(PathBuf::from(source)),
                PackageSource::Path,
            ),
            Ok(url) if url.scheme().len() > 1 => PackageSource::Url(url),
            _ => PackageSource::Path(PathBuf::from(source)),
        }
    }

    /// Whether opening this source requires an HTTP client.
    pub fn is_remote(&self) -> bool {
        matches!(self, PackageSource::Url(_))
    }
}

/// Opens a package archive from a source. `client` is only required for
/// remote sources; `display` is the user-provided form of the source used in
/// error messages.
pub async fn open_package(
    source: &PackageSource,
    client: Option<reqwest_middleware::ClientWithMiddleware>,
    display: &str,
) -> miette::Result<PackageArchive> {
    let archive = match source {
        PackageSource::Url(url) => {
            let client = client.expect("a client is created whenever a source is a URL");
            PackageArchive::from_url(client, url.clone()).await
        }
        PackageSource::Path(path) => PackageArchive::from_path(path).await,
    };
    archive
        .into_diagnostic()
        .with_context(|| format!("failed to open package {display}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_source() {
        assert!(matches!(
            PackageSource::parse("https://example.com/pkg-1.0-0.conda"),
            PackageSource::Url(_)
        ));
        assert!(matches!(
            PackageSource::parse("./pkg-1.0-0.conda"),
            PackageSource::Path(_)
        ));
        assert!(matches!(
            PackageSource::parse("pkg-1.0-0.tar.bz2"),
            PackageSource::Path(_)
        ));
        // A single letter scheme is a Windows drive letter, not a URL.
        assert!(matches!(
            PackageSource::parse(r"C:\packages\pkg-1.0-0.conda"),
            PackageSource::Path(_)
        ));
    }
}
