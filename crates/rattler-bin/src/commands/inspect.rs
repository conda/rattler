use miette::{Context, IntoDiagnostic};
use rattler_conda_types::package::{IndexJson, PackageFile, PathsJson};
use rattler_package_streaming::reqwest::fetch::fetch_package_file_from_remote_url;
use rattler_package_streaming::seek::read_package_file;
use url::Url;

/// Inspect package metadata from a local or remote conda package.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Path or URL to the conda package to inspect (.conda or .tar.bz2 archive)
    #[clap(required = true)]
    package: String,

    /// Number of files to print
    #[clap(long, default_value_t = 10)]
    limit: usize,
}

/// Reads a typed package file from either a remote URL or a local path.
async fn read_file<P: PackageFile + Send + 'static>(package: &str) -> miette::Result<P> {
    match parse_remote_url(package) {
        Some(url) => {
            let client = super::client::create_client_with_middleware()?;
            fetch_package_file_from_remote_url(client, url)
                .await
                .into_diagnostic()
        }
        None => {
            let package = package.to_string();
            tokio::task::spawn_blocking(move || read_package_file::<P>(&package))
                .await
                .into_diagnostic()?
                .into_diagnostic()
        }
    }
}

/// Parses the argument as a remote URL, returning `None` for local paths
/// (including `file://` URLs).
fn parse_remote_url(package: &str) -> Option<Url> {
    match Url::parse(package) {
        Ok(url) if url.scheme() != "file" => Some(url),
        _ => None,
    }
}

pub async fn inspect(opt: Opt) -> miette::Result<()> {
    let index_json: IndexJson = read_file(&opt.package)
        .await
        .context("failed to read index.json")?;

    println!("name: {}", index_json.name.as_normalized());
    println!("version: {}", index_json.version);
    println!("build: {}", index_json.build);
    if let Some(ref license) = index_json.license {
        println!("license: {license}");
    }
    if let Some(ref subdir) = index_json.subdir {
        println!("subdir: {subdir}");
    }
    if !index_json.depends.is_empty() {
        println!("depends:");
        for dep in &index_json.depends {
            println!("  - {dep}");
        }
    }
    if !index_json.constrains.is_empty() {
        println!("constrains:");
        for c in &index_json.constrains {
            println!("  - {c}");
        }
    }

    let paths_json: PathsJson = read_file(&opt.package)
        .await
        .context("failed to read paths.json")?;

    let total = paths_json.paths.len();
    println!("paths: ({total} total)");
    for entry in paths_json.paths.iter().take(opt.limit) {
        println!("  - {}", entry.relative_path.display());
    }
    if total > opt.limit {
        println!("  ... and {} more", total - opt.limit);
    }

    Ok(())
}
