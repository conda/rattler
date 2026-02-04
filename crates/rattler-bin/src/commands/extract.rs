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

    /// Path to a Content Addressable Store (CAS) directory for file deduplication.
    /// When specified, file contents are stored in the CAS and hardlinked to the destination.
    #[clap(long)]
    cas: Option<PathBuf>,
}

/// Strips package extensions (.tar.bz2 or .conda) from a filename
fn strip_package_extension(filename: &str) -> String {
    if let Some(stripped) = filename.strip_suffix(".tar.bz2") {
        stripped.to_string()
    } else if let Some(stripped) = filename.strip_suffix(".conda") {
        stripped.to_string()
    } else {
        filename.to_string()
    }
}

/// Creates an HTTP client with authentication middleware
fn create_authenticated_client() -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    let download_client = Client::builder()
        .no_gzip()
        .build()
        .into_diagnostic()
        .context("Failed to create HTTP client")?;

    let authentication_storage =
        AuthenticationStorage::from_env_and_defaults().into_diagnostic()?;

    let client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
            authentication_storage.clone(),
        )))
        .with(rattler_networking::OciMiddleware);
    #[cfg(feature = "s3")]
    let client = client.with(rattler_networking::S3Middleware::new(
        std::collections::HashMap::new(),
        authentication_storage,
    ));
    #[cfg(feature = "gcs")]
    let client = client.with(rattler_networking::GCSMiddleware);
    let client = client.build();

    Ok(client)
}

/// Determines the destination directory from a URL
fn determine_destination_from_url(url: &Url) -> miette::Result<PathBuf> {
    // Extract filename from URL path
    let filename = url
        .path_segments()
        .and_then(Iterator::last)
        .ok_or_else(|| miette::miette!("Could not extract package name from URL"))?;

    let package_name = strip_package_extension(filename);
    Ok(PathBuf::from(package_name))
}

/// Extracts a conda package from a URL
async fn extract_from_url(
    url: Url,
    destination: Option<PathBuf>,
    cas: Option<PathBuf>,
    package_display: &str,
) -> miette::Result<(PathBuf, rattler_package_streaming::ExtractResult)> {
    let destination = destination.map_or_else(|| determine_destination_from_url(&url), Ok)?;

    println!(
        "Extracting {} to {}",
        package_display,
        destination.display()
    );

    let client = create_authenticated_client()?;

    let result = rattler_package_streaming::reqwest::tokio::extract(
        client,
        url,
        &destination,
        cas.as_deref(),
        None,
        None,
    )
    .await
    .into_diagnostic()
    .with_context(|| format!("Failed to extract package from URL: {package_display}"))?;

    Ok((destination, result))
}

/// Determines the destination directory from a file path
fn determine_destination_from_path(package_path: &str) -> miette::Result<PathBuf> {
    let path = PathBuf::from(package_path);
    let package_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| miette::miette!("Invalid package filename"))?
        .to_string();

    Ok(PathBuf::from(package_name))
}

/// Extracts a conda package from a local file path
fn extract_from_path(
    package_path: &str,
    destination: Option<PathBuf>,
    cas: Option<PathBuf>,
) -> miette::Result<(PathBuf, rattler_package_streaming::ExtractResult)> {
    let destination =
        destination.map_or_else(|| determine_destination_from_path(package_path), Ok)?;

    println!("Extracting {} to {}", package_path, destination.display());

    let result = rattler_package_streaming::fs::extract(
        &PathBuf::from(package_path),
        &destination,
        cas.as_deref(),
    )
    .into_diagnostic()
    .with_context(|| format!("Failed to extract package: {package_path}"))?;

    Ok((destination, result))
}

pub async fn extract(opt: Opt) -> miette::Result<()> {
    // Try to parse as URL, otherwise treat as file path
    let (destination, result) = if let Ok(url) = Url::parse(&opt.package) {
        extract_from_url(url, opt.destination, opt.cas, &opt.package).await?
    } else {
        extract_from_path(&opt.package, opt.destination, opt.cas)?
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
