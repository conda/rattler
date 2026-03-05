use std::sync::Arc;

use miette::{Context, IntoDiagnostic};
use rattler_conda_types::package::IndexJson;
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage};
use rattler_package_streaming::reqwest::fetch::fetch_package_file_from_url;
use reqwest::Client;
use url::Url;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// URL of the conda package to inspect (must be a .conda archive)
    #[clap(required = true)]
    url: Url,
}

pub async fn inspect(opt: Opt) -> miette::Result<()> {
    let download_client = Client::builder()
        .no_gzip()
        .build()
        .into_diagnostic()
        .context("failed to create HTTP client")?;

    let authentication_storage =
        AuthenticationStorage::from_env_and_defaults().into_diagnostic()?;

    let client = reqwest_middleware::ClientBuilder::new(download_client.clone())
        .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
            authentication_storage,
        )))
        .build();

    let index_json: IndexJson = fetch_package_file_from_url(client, opt.url)
        .await
        .into_diagnostic()
        .context("failed to fetch package info")?;

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

    Ok(())
}
