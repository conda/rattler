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

pub async fn unpack(opt: Opt) -> miette::Result<()> {
    // Try to parse as URL, otherwise treat as file path
    let is_url = Url::parse(&opt.package).is_ok();

    // Determine the destination directory
    let destination = if let Some(dest) = opt.destination {
        dest
    } else {
        // Extract to a directory with the same name as the package (without extension)
        let package_name: String = if is_url {
            // Extract filename from URL path
            let url = Url::parse(&opt.package).into_diagnostic()?;
            let filename = url
                .path_segments()
                .and_then(Iterator::last)
                .ok_or_else(|| miette::miette!("Could not extract package name from URL"))?;

            // Remove extensions (.tar.bz2 or .conda)
            if let Some(stripped) = filename.strip_suffix(".tar.bz2") {
                stripped.to_string()
            } else if let Some(stripped) = filename.strip_suffix(".conda") {
                stripped.to_string()
            } else {
                filename.to_string()
            }
        } else {
            let path = PathBuf::from(&opt.package);
            path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| miette::miette!("Invalid package filename"))?
                .to_string()
        };

        PathBuf::from(package_name)
    };

    println!("Extracting {} to {}", opt.package, destination.display());

    // Extract the package
    let result = if is_url {
        // Create HTTP client with authentication middleware
        let download_client = Client::builder()
            .no_gzip()
            .build()
            .into_diagnostic()
            .context("Failed to create HTTP client")?;

        let authentication_storage = AuthenticationStorage::empty();
        let download_client = reqwest_middleware::ClientBuilder::new(download_client)
            .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
                authentication_storage,
            )))
            .with(rattler_networking::OciMiddleware)
            .with(rattler_networking::GCSMiddleware)
            .build();

        let url = Url::parse(&opt.package)
            .into_diagnostic()
            .context("Invalid URL")?;

        rattler_package_streaming::reqwest::tokio::extract(
            download_client,
            url,
            &destination,
            None,
            None,
        )
        .await
        .into_diagnostic()
        .with_context(|| format!("Failed to extract package from URL: {}", opt.package))?
    } else {
        rattler_package_streaming::fs::extract(&PathBuf::from(&opt.package), &destination)
            .into_diagnostic()
            .with_context(|| format!("Failed to extract package: {}", opt.package))?
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
