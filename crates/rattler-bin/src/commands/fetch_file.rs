use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use miette::{Context, IntoDiagnostic};
use rattler_conda_types::package::CondaArchiveType;
use rattler_package_streaming::reqwest::fetch::fetch_file_from_remote_url;
use rattler_package_streaming::seek::read_package_file_content;
use url::Url;

/// Read a file from inside a local or remote conda package.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Path or URL to the conda package (.conda or .tar.bz2 archive)
    #[clap(required = true)]
    package: String,

    /// Path of the file inside the package (e.g. "info/index.json" or "lib/libfoo.so")
    #[clap(required = true)]
    path: String,
}

pub async fn fetch_file(opt: Opt, offline: bool) -> miette::Result<()> {
    let Opt { package, path } = opt;

    let target_path = Path::new(&path);

    let bytes = match parse_remote_url(&package) {
        Some(url) => {
            let client = super::client::create_client_with_middleware(offline)?;
            fetch_file_from_remote_url(client, url, target_path)
                .await
                .into_diagnostic()?
        }
        None => read_file_from_local_package(&package, target_path)?,
    };

    let bytes = bytes.ok_or_else(|| miette::miette!("file '{}' not found in package", path))?;

    std::io::stdout()
        .write_all(&bytes)
        .into_diagnostic()
        .context("failed to write to stdout")?;
    Ok(())
}

/// Parses the argument as a remote URL, returning `None` for local paths
/// (including `file://` URLs).
fn parse_remote_url(package: &str) -> Option<Url> {
    match Url::parse(package) {
        Ok(url) if url.scheme() != "file" => Some(url),
        _ => None,
    }
}

/// Reads a file from inside a local conda package archive.
fn read_file_from_local_package(
    package: &str,
    target_path: &Path,
) -> miette::Result<Option<Vec<u8>>> {
    let package_path = PathBuf::from(package);
    let archive_type = CondaArchiveType::try_from(&package_path)
        .ok_or_else(|| miette::miette!("'{package}' is not a .conda or .tar.bz2 archive"))?;
    let file = File::open(&package_path)
        .into_diagnostic()
        .with_context(|| format!("failed to open {package}"))?;

    match read_package_file_content(&file, archive_type, target_path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(rattler_package_streaming::ExtractError::MissingComponent) => Ok(None),
        Err(err) => Err(err)
            .into_diagnostic()
            .with_context(|| format!("failed to read '{}' from {package}", target_path.display())),
    }
}
