use miette::{Context, IntoDiagnostic};
use rattler_conda_types::package::{IndexJson, PathsJson};
use rattler_package_streaming::reqwest::fetch::fetch_package_file_from_remote_url;
use url::Url;

/// Inspect package metadata from a remote conda package.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// URL of the conda package to inspect (must be a .conda archive)
    #[clap(required = true)]
    url: Url,
}

pub async fn inspect(opt: Opt) -> miette::Result<()> {
    let client = super::client::create_client_with_middleware()?;

    let index_json: IndexJson = fetch_package_file_from_remote_url(client.clone(), opt.url.clone())
        .await
        .into_diagnostic()
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

    let paths_json: PathsJson = fetch_package_file_from_remote_url(client, opt.url)
        .await
        .into_diagnostic()
        .context("failed to read paths.json")?;

    let total = paths_json.paths.len();
    println!("paths: ({total} total)");
    for entry in paths_json.paths.iter().take(10) {
        println!("  - {}", entry.relative_path.display());
    }
    if total > 10 {
        println!("  ... and {} more", total - 10);
    }

    Ok(())
}
