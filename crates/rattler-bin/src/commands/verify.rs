//! Verify sigstore signatures for conda packages

use std::{path::PathBuf, sync::Arc};

use chrono::DateTime;
use miette::{Context, IntoDiagnostic};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage};
use reqwest::Client;
use sigstore_trust_root::TrustedRoot;
use sigstore_verify::{types::Bundle, VerificationPolicy, Verifier};
use url::Url;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// URL or path to the conda package to verify (e.g., https://prefix.dev/channel/linux-64/pkg-1.0.0-h123_0.conda)
    #[clap(required = true)]
    package: String,

    /// Optional URL to the signatures file. If not provided, appends `.v0.sigs` to the package URL.
    #[clap(short, long)]
    signatures_url: Option<String>,

    /// Required identity (SAN) from the signing certificate (e.g., workflow identity URI)
    #[clap(long)]
    identity: Option<String>,

    /// Required issuer (e.g., https://token.actions.githubusercontent.com)
    #[clap(long)]
    issuer: Option<String>,
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
            authentication_storage,
        )))
        .with(rattler_networking::OciMiddleware)
        .with(rattler_networking::GCSMiddleware)
        .build();

    Ok(client)
}

/// Fetch signatures from URL
async fn fetch_signatures(
    client: &reqwest_middleware::ClientWithMiddleware,
    url: &Url,
) -> miette::Result<Vec<Bundle>> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .into_diagnostic()
        .with_context(|| format!("Failed to fetch signatures from {url}"))?;

    if !response.status().is_success() {
        return Err(miette::miette!(
            "Failed to fetch signatures: HTTP {}",
            response.status()
        ));
    }

    let body = response
        .text()
        .await
        .into_diagnostic()
        .context("Failed to read signatures response body")?;

    // Parse as JSON array of bundles
    let bundles_json: Vec<serde_json::Value> = serde_json::from_str(&body)
        .into_diagnostic()
        .context("Failed to parse signatures as JSON array")?;

    let mut bundles = Vec::new();
    for (i, bundle_json) in bundles_json.iter().enumerate() {
        let bundle_str = serde_json::to_string(bundle_json).into_diagnostic()?;
        let bundle = Bundle::from_json(&bundle_str)
            .map_err(|e| miette::miette!("Failed to parse bundle {}: {}", i, e))?;
        bundles.push(bundle);
    }

    if bundles.is_empty() {
        return Err(miette::miette!("No signatures found in signatures file"));
    }

    Ok(bundles)
}

/// Compute SHA256 digest of a file
fn compute_file_digest(path: &std::path::Path) -> miette::Result<String> {
    let digest = rattler_digest::compute_file_digest::<rattler_digest::Sha256>(path)
        .into_diagnostic()
        .with_context(|| format!("Failed to compute digest for {}", path.display()))?;
    Ok(format!("{digest:x}"))
}

/// Compute SHA256 digest of bytes
fn compute_bytes_digest(bytes: &[u8]) -> String {
    use rattler_digest::Sha256;
    use sha2::Digest;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    format!("{result:x}")
}

/// Download package and return bytes
async fn download_package(
    client: &reqwest_middleware::ClientWithMiddleware,
    url: &Url,
) -> miette::Result<Vec<u8>> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .into_diagnostic()
        .with_context(|| format!("Failed to download package from {url}"))?;

    if !response.status().is_success() {
        return Err(miette::miette!(
            "Failed to download package: HTTP {}",
            response.status()
        ));
    }

    response
        .bytes()
        .await
        .into_diagnostic()
        .map(|b| b.to_vec())
        .context("Failed to read package bytes")
}

pub async fn verify(opt: Opt) -> miette::Result<()> {
    let client = create_authenticated_client()?;

    // Determine if package is URL or local path
    let (package_bytes, package_digest, package_name) = if let Ok(url) = Url::parse(&opt.package) {
        println!("Downloading package from {}...", url);
        let bytes = download_package(&client, &url).await?;
        let digest = compute_bytes_digest(&bytes);
        let name = url
            .path_segments()
            .and_then(Iterator::last)
            .unwrap_or("unknown")
            .to_string();
        (bytes, digest, name)
    } else {
        let path = PathBuf::from(&opt.package);
        if !path.exists() {
            return Err(miette::miette!("Package file not found: {}", path.display()));
        }
        println!("Reading local package {}...", path.display());
        let digest = compute_file_digest(&path)?;
        let bytes = std::fs::read(&path)
            .into_diagnostic()
            .with_context(|| format!("Failed to read package file: {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        (bytes, digest, name)
    };

    println!("Package: {}", package_name);
    println!("SHA256:  {}", package_digest);

    // Determine signatures URL
    let signatures_url = if let Some(sig_url) = opt.signatures_url {
        Url::parse(&sig_url)
            .into_diagnostic()
            .context("Invalid signatures URL")?
    } else if let Ok(url) = Url::parse(&opt.package) {
        // Append .v0.sigs to the package URL
        let sig_url_str = format!("{}.v0.sigs", url);
        Url::parse(&sig_url_str)
            .into_diagnostic()
            .context("Failed to construct signatures URL")?
    } else {
        return Err(miette::miette!(
            "Cannot determine signatures URL for local file. Please provide --signatures-url"
        ));
    };

    println!("Fetching signatures from {}...", signatures_url);
    let bundles = fetch_signatures(&client, &signatures_url).await?;
    println!("Found {} signature(s)", bundles.len());

    // Load trusted root (production Sigstore)
    println!("Loading Sigstore trusted root...");
    let trusted_root = TrustedRoot::production()
        .map_err(|e| miette::miette!("Failed to load Sigstore trusted root: {}", e))?;

    // Build verification policy
    let mut policy = VerificationPolicy::default();
    if let Some(ref identity) = opt.identity {
        policy = policy.require_identity(identity);
        println!("Requiring identity: {}", identity);
    }
    if let Some(ref issuer) = opt.issuer {
        policy = policy.require_issuer(issuer);
        println!("Requiring issuer: {}", issuer);
    }

    // Create verifier
    let verifier = Verifier::new(&trusted_root);

    // Verify each bundle
    let mut any_valid = false;
    for (i, bundle) in bundles.iter().enumerate() {
        print!("Verifying signature {}... ", i + 1);

        match verifier.verify(&package_bytes, bundle, &policy) {
            Ok(result) => {
                if result.success {
                    println!("{} Valid!", console::style("✓").green());
                    any_valid = true;

                    // Print identity info if available
                    if let Some(identity) = &result.identity {
                        println!("  Identity: {}", identity);
                    }
                    if let Some(issuer) = &result.issuer {
                        println!("  Issuer:   {}", issuer);
                    }
                    if let Some(time) = result.integrated_time {
                        println!("  Signed:   {}", DateTime::from_timestamp(time, 0)
                            .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                            .unwrap_or_else(|| time.to_string()));
                    }
                    if !result.warnings.is_empty() {
                        for warning in &result.warnings {
                            println!("  Warning:  {}", console::style(warning).yellow());
                        }
                    }
                } else {
                    println!("{} Invalid", console::style("✗").red());
                }
            }
            Err(e) => {
                println!("{} Error: {}", console::style("✗").red(), e);
            }
        }
    }

    if any_valid {
        println!(
            "\n{} Package signature verified successfully!",
            console::style("✓").green()
        );
        Ok(())
    } else {
        Err(miette::miette!(
            "No valid signatures found for this package"
        ))
    }
}
