use std::{path::PathBuf, sync::Arc};

use miette::{Context, IntoDiagnostic};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage};
use reqwest::Client;
use url::Url;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Path or URL to the conda package archive (.tar.bz2 or .conda)
    #[clap(required = true)]
    package: String,

    /// Destination directory where the package will be extracted
    /// If not specified, extracts to a directory with the same name as the package
    #[clap(short, long)]
    destination: Option<PathBuf>,
}

pub async fn extract(opt: Opt) -> miette::Result<()> {
    // Try to parse as URL, otherwise treat as file path
    let (destination, result) = if let Ok(url) = Url::parse(&opt.package) {
        // URL path: download and extract from remote location
        let destination = if let Some(dest) = opt.destination {
            dest
        } else {
            // Extract filename from URL path
            let filename = url
                .path_segments()
                .and_then(Iterator::last)
                .ok_or_else(|| miette::miette!("Could not extract package name from URL"))?;

            // Remove extensions (.tar.bz2 or .conda)
            let package_name = if let Some(stripped) = filename.strip_suffix(".tar.bz2") {
                stripped.to_string()
            } else if let Some(stripped) = filename.strip_suffix(".conda") {
                stripped.to_string()
            } else {
                filename.to_string()
            };

            PathBuf::from(package_name)
        };

        println!("Extracting {} to {}", opt.package, destination.display());

        // Create HTTP client with authentication middleware
        let download_client = Client::builder()
            .no_gzip()
            .build()
            .into_diagnostic()
            .context("Failed to create HTTP client")?;

        let authentication_storage =
            AuthenticationStorage::from_env_and_defaults().into_diagnostic()?;
        let download_client = reqwest_middleware::ClientBuilder::new(download_client)
            .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
                authentication_storage,
            )))
            .with(rattler_networking::OciMiddleware)
            .with(rattler_networking::GCSMiddleware)
            .build();

        let result = rattler_package_streaming::reqwest::tokio::extract(
            download_client,
            url,
            &destination,
            None,
            None,
        )
        .await
        .into_diagnostic()
        .with_context(|| format!("Failed to extract package from URL: {}", opt.package))?;

        (destination, result)
    } else {
        // File path: extract from local file
        let destination = if let Some(dest) = opt.destination {
            dest
        } else {
            // Extract to a directory with the same name as the package (without extension)
            let path = PathBuf::from(&opt.package);
            let package_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| miette::miette!("Invalid package filename"))?
                .to_string();

            PathBuf::from(package_name)
        };

        println!("Extracting {} to {}", opt.package, destination.display());

        let result =
            rattler_package_streaming::fs::extract(&PathBuf::from(&opt.package), &destination)
                .into_diagnostic()
                .with_context(|| format!("Failed to extract package: {}", opt.package))?;

        (destination, result)
    };

    println!(
        "{} Successfully extracted package",
        console::style("✓").green(),
    );
    println!("  Destination: {}", destination.display());
    println!("  SHA256: {:x}", result.sha256);
    println!("  MD5: {:x}", result.md5);
    println!("  Size: {} bytes", result.total_size);

    Ok(())
}
