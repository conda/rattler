//! Verify sigstore signatures for conda packages.
//!
//! This is a thin CLI wrapper around the [`rattler_sigstore`] crate: it
//! resolves the package bytes (from a URL or a local file), builds a
//! verification policy from the command-line options and delegates the actual
//! signature verification to [`rattler_sigstore::verify_package`].

use std::path::PathBuf;

use miette::{Context, IntoDiagnostic};
use rattler_sigstore::{Publisher, VerificationConfig, VerificationPolicy, verify_package};
use url::Url;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// URL or path to the conda package to verify (e.g., <https://prefix.dev/channel/linux-64/pkg-1.0.0-h123_0.conda>)
    #[clap(required = true)]
    package: String,

    /// Optional URL to the signatures file. If not provided, appends `.v0.sigs` to the package URL.
    #[clap(short, long)]
    signatures_url: Option<String>,

    /// Required identity (SAN) from the signing certificate (e.g., workflow identity URI)
    #[clap(long)]
    identity: Option<String>,

    /// Required issuer (e.g., <https://token.actions.githubusercontent.com>)
    #[clap(long)]
    issuer: Option<String>,
}

/// Creates an HTTP client with authentication middleware
fn create_authenticated_client() -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    super::client::create_client_with_middleware()
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

    // Resolve the package bytes, a URL to associate with the package and a
    // human-readable name. The URL is used by `rattler_sigstore` to derive the
    // signatures URL (unless an explicit one is provided below).
    let (package_bytes, package_url, package_name) = if let Ok(url) = Url::parse(&opt.package) {
        println!("Downloading package from {url}...");
        let bytes = download_package(&client, &url).await?;
        let name = url
            .path_segments()
            .and_then(Iterator::last)
            .unwrap_or("unknown")
            .to_string();
        (bytes, url, name)
    } else {
        let path = PathBuf::from(&opt.package);
        if !path.exists() {
            return Err(miette::miette!(
                "Package file not found: {}",
                path.display()
            ));
        }
        if opt.signatures_url.is_none() {
            return Err(miette::miette!(
                "Cannot determine signatures URL for a local file. Please provide --signatures-url"
            ));
        }
        println!("Reading local package {}...", path.display());
        let bytes = std::fs::read(&path)
            .into_diagnostic()
            .with_context(|| format!("Failed to read package file: {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let absolute = std::fs::canonicalize(&path)
            .into_diagnostic()
            .with_context(|| format!("Failed to resolve path: {}", path.display()))?;
        let url = Url::from_file_path(&absolute)
            .map_err(|()| miette::miette!("Failed to build URL for {}", absolute.display()))?;
        (bytes, url, name)
    };

    println!("Package: {package_name}");

    // Build a single-publisher policy from the optional identity/issuer
    // constraints. An empty publisher (no constraints) accepts any valid
    // signature, matching the previous behaviour of this command.
    let mut publisher = Publisher::new();
    if let Some(identity) = &opt.identity {
        publisher = publisher.with_identity(identity.clone());
        println!("Requiring identity: {identity}");
    }
    if let Some(issuer) = &opt.issuer {
        publisher = publisher.with_issuer(issuer.clone());
        println!("Requiring issuer: {issuer}");
    }

    let mut config = VerificationConfig::new().with_default_publishers(vec![publisher]);
    if let Some(signatures_url) = &opt.signatures_url {
        config = config.with_signatures_url_pattern(signatures_url.clone());
    }
    // `Require` makes verification failures hard errors (non-zero exit).
    let policy = VerificationPolicy::Require(config);

    println!("Verifying signatures...");
    let outcome = verify_package(&policy, &package_url, &package_bytes, &client)
        .await
        .into_diagnostic()
        .context("Signature verification failed")?;

    // In `Require` mode a non-verified outcome is reported as an error above,
    // so reaching this point means verification succeeded.
    debug_assert!(outcome.verified);

    println!(
        "\n{} Package signature verified successfully!",
        console::style("✓").green()
    );
    if let Some(identity) = &outcome.identity {
        println!("  Identity: {identity}");
    }
    if let Some(issuer) = &outcome.issuer {
        println!("  Issuer:   {issuer}");
    }
    if let Some(signed_at) = outcome.signed_at {
        println!("  Signed:   {signed_at}");
    }
    for warning in &outcome.warnings {
        println!("  {} {warning}", console::style("Warning:").yellow());
    }

    Ok(())
}
