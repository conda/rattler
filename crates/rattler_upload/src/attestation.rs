//! Sigstore attestation support for conda packages
//!
//! This module provides functionality to create Sigstore attestations for conda packages
//! using the sigstore-rs library. It supports both interactive OAuth flows and GitHub Actions
//! OIDC tokens for authentication.

use miette::IntoDiagnostic;
use serde_json::json;
use sha2::{Digest, Sha256};
use sigstore::bundle::intoto::{StatementBuilder, Subject};
use sigstore::bundle::sign::SigningContext;
use sigstore::oauth;
use std::env;
use std::path::Path;
use tracing::{info, warn};

/// The predicate type URI for conda package attestations
pub const CONDA_ATTESTATION_PREDICATE_TYPE: &str =
    "https://schemas.conda.org/attestations-publish-1.schema.json";

/// Creates a Sigstore attestation bundle for a conda package
///
/// # Arguments
///
/// * `package_path` - Path to the conda package file
/// * `channel_url` - The target channel URL where the package will be uploaded
///
/// # Returns
///
/// Returns a JSON string containing the Sigstore bundle in v0.3 format with DSSE envelope
pub async fn create_attestation(package_path: &Path, channel_url: &str) -> miette::Result<String> {
    info!(
        "Creating attestation for package: {}",
        package_path.display()
    );

    // Read and hash the package file
    let package_bytes = tokio::fs::read(package_path).await.into_diagnostic()?;
    let mut hasher = Sha256::new();
    hasher.update(&package_bytes);
    let digest = hasher.finalize();
    let digest_hex = hex::encode(digest);

    info!("  SHA256: {}", digest_hex);

    // Get the package filename
    let package_name = package_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| miette::miette!("Invalid package filename"))?;

    // Create the subject (the package being attested)
    let subject = Subject::new(package_name, "sha256", &digest_hex);

    // Create the predicate with conda-specific metadata
    let predicate = json!({
        "targetChannel": channel_url,
    });

    // Build the in-toto statement
    let statement = StatementBuilder::new()
        .subject(subject)
        .predicate_type(CONDA_ATTESTATION_PREDICATE_TYPE)
        .predicate(predicate)
        .build()
        .map_err(|e| miette::miette!("Failed to build attestation statement: {}", e))?;

    info!("Statement created:");
    info!("  Type: {}", statement.statement_type);
    info!("  Predicate Type: {}", statement.predicate_type);
    info!("  Subjects: {}", statement.subject.len());

    // Get identity token (GitHub Actions OIDC or interactive OAuth)
    info!("Authenticating with Sigstore...");
    let token = get_identity_token().await?;

    // Create signing context and sign the statement in a blocking task
    info!("Connecting to Sigstore production instance...");
    info!("Creating DSSE envelope and signing...");

    let bundle_json = tokio::task::spawn_blocking(move || -> miette::Result<String> {
        let ctx = SigningContext::production().into_diagnostic()?;
        let signer = ctx.blocking_signer(token).into_diagnostic()?;
        let artifact = signer.sign_dsse(&statement).into_diagnostic()?;

        // Create and serialize the bundle
        let bundle = artifact.to_bundle();
        serde_json::to_string_pretty(&bundle).into_diagnostic()
    })
    .await
    .into_diagnostic()??;

    info!("✓ Successfully created attestation bundle");

    Ok(bundle_json)
}

/// Gets an identity token for Sigstore authentication
///
/// This function tries the following methods in order:
/// 1. GitHub Actions OIDC token (via environment variables)
/// 2. Interactive OAuth flow (if not in CI/non-interactive mode)
async fn get_identity_token() -> miette::Result<oauth::IdentityToken> {
    // Check for GitHub Actions OIDC token
    if let (Ok(token_url), Ok(request_token)) = (
        env::var("ACTIONS_ID_TOKEN_REQUEST_URL"),
        env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN"),
    ) {
        info!("  Using GitHub Actions OIDC token");

        let client = reqwest::Client::new();
        let response = client
            .get(&format!("{}&audience=sigstore", token_url))
            .header("Authorization", format!("Bearer {}", request_token))
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette::miette!(
                "Failed to get OIDC token from GitHub Actions: {}",
                response.status()
            ));
        }

        let token_response: serde_json::Value = response.json().await.into_diagnostic()?;
        let token_string = token_response["value"]
            .as_str()
            .ok_or_else(|| miette::miette!("Missing 'value' field in token response"))?;

        return Ok(token_string.try_into().into_diagnostic()?);
    }

    // Check for COSIGN_YES environment variable to skip interactive flow
    if env::var("COSIGN_YES").is_ok() {
        return Err(miette::miette!(
            "No OIDC token available and COSIGN_YES is set (non-interactive mode)"
        ));
    }

    // Fall back to interactive OAuth flow
    info!("  Using interactive OAuth flow...");
    info!("  A browser window will open for authentication");

    let oidc_url = oauth::openidflow::OpenIDAuthorize::new(
        "sigstore",
        "",
        "https://oauth2.sigstore.dev/auth",
        "http://localhost:8080",
    )
    .auth_url()
    .into_diagnostic()?;

    info!("  Opening browser to: {}", oidc_url.0.as_ref());

    // Spawn browser opening in a separate task to not block
    let url_clone = oidc_url.0.as_ref().to_string();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = webbrowser::open(&url_clone) {
            warn!("Failed to open browser: {}", e);
        }
    });

    info!("  Waiting for authentication...");

    // Run the redirect listener in a blocking context
    let state = oidc_url.1;
    let nonce = oidc_url.2;
    let pkce_verifier = oidc_url.3;

    let (_, token) = tokio::task::spawn_blocking(move || {
        let listener =
            oauth::openidflow::RedirectListener::new("127.0.0.1:8080", state, nonce, pkce_verifier);
        listener.redirect_listener()
    })
    .await
    .into_diagnostic()?
    .into_diagnostic()?;

    Ok(oauth::IdentityToken::from(token))
}
