use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use miette::{Context, IntoDiagnostic};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage};
use rattler_package_streaming::reqwest::sparse::fetch_file_from_remote_conda;
use reqwest::Client;
use url::Url;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// URL of the conda package (.conda archive)
    #[clap(required = true)]
    url: Url,

    /// Path of the file inside the package (e.g. "info/index.json" or "lib/libfoo.so")
    #[clap(required = true)]
    path: String,
}

pub async fn fetch_file(opt: Opt) -> miette::Result<()> {
    let download_client = Client::builder()
        .no_gzip()
        .build()
        .into_diagnostic()
        .context("failed to create HTTP client")?;

    let authentication_storage =
        AuthenticationStorage::from_env_and_defaults().into_diagnostic()?;

    let client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
            authentication_storage,
        )))
        .build();

    let target_path = Path::new(&opt.path);

    let bytes = fetch_file_from_remote_conda(client, opt.url, target_path)
        .await
        .into_diagnostic()
        .context("failed to fetch file from package")?
        .ok_or_else(|| miette::miette!("file '{}' not found in package", opt.path))?;

    std::io::stdout()
        .write_all(&bytes)
        .into_diagnostic()
        .context("failed to write to stdout")?;

    Ok(())
}
