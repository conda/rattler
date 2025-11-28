//! Native Sigstore attestation creation for conda packages
//!
//! This module provides attestation creation using the sigstore-sign crate,
//! replacing the previous cosign CLI-based implementation.

use miette::IntoDiagnostic;
use reqwest::header;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sigstore_sign::{oidc::IdentityToken, Attestation, SigningContext};
use std::path::Path;

/// Conda V1 predicate
#[derive(Debug, Serialize, Deserialize)]
pub struct CondaV1Predicate {
    #[serde(rename = "targetChannel", skip_serializing_if = "Option::is_none")]
    pub target_channel: Option<String>,
}

/// Response from GitHub attestation API
#[derive(Debug, Serialize, Deserialize)]
pub struct AttestationResponse {
    pub id: String,
}

/// Configuration for attestation creation
#[derive(Debug, Clone)]
pub struct AttestationConfig {
    pub repo_owner: Option<String>,
    pub repo_name: Option<String>,
    pub github_token: Option<String>,
}

/// Create and store an attestation for a conda package using native Sigstore signing
///
/// This function:
/// 1. Creates an in-toto statement for the package
/// 2. Uses Sigstore (Fulcio + Rekor) to sign the statement with OIDC identity
/// 3. Optionally stores the signed attestation to GitHub's attestation API (if token provided)
///
/// Returns the attestation bundle JSON or GitHub attestation ID
pub async fn create_attestation(
    package_path: &Path,
    channel_url: &str,
    config: &AttestationConfig,
    client: &ClientWithMiddleware,
) -> miette::Result<String> {
    // Step 1: Get identity token from ambient environment (GitHub Actions, GitLab CI, etc.)
    let identity_token = get_identity_token().await?;

    // Step 2: Compute package digest
    let digest = rattler_digest::compute_file_digest::<rattler_digest::Sha256>(package_path)
        .into_diagnostic()?;
    let digest_hex = format!("{digest:x}");

    // Step 3: Get package filename
    let filename = package_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| miette::miette!("Invalid package file name"))?;

    // Step 4: Create attestation with in-toto statement
    let predicate = CondaV1Predicate {
        target_channel: Some(channel_url.to_string()),
    };

    let sha256_hash = sigstore_sign::types::Sha256Hash::from_hex(&digest_hex)
        .map_err(|e| miette::miette!("Invalid SHA256 hash: {}", e))?;

    let attestation = Attestation::new(
        "https://schemas.conda.org/attestations-publish-1.schema.json",
        serde_json::to_value(&predicate).into_diagnostic()?,
    )
    .add_subject(filename, sha256_hash);

    // Step 5: Sign with Sigstore
    tracing::info!("Signing attestation with Sigstore...");
    let context = SigningContext::production();
    let signer = context.signer(identity_token);

    let bundle = signer
        .sign_attestation(attestation)
        .await
        .map_err(|e| miette::miette!("Failed to sign attestation with Sigstore: {}", e))?;

    let bundle_json = bundle
        .to_json_pretty()
        .map_err(|e| miette::miette!("Failed to serialize bundle: {}", e))?;

    tracing::info!("Successfully created Sigstore attestation");

    // Step 6: Optionally store to GitHub if token is provided
    if let (Some(token), Some(owner), Some(repo)) =
        (&config.github_token, &config.repo_owner, &config.repo_name)
    {
        let attestation_id =
            store_attestation_to_github(&bundle_json, token, owner, repo, client).await?;

        tracing::info!("Attestation stored to GitHub with ID: {}", attestation_id);
        Ok(attestation_id)
    } else {
        tracing::info!("GitHub token not provided, returning bundle for direct upload");
        Ok(bundle_json)
    }
}

/// Get an identity token from the ambient CI/CD environment
async fn get_identity_token() -> miette::Result<IdentityToken> {
    use sigstore_sign::oidc::ambient;

    // Check if we're in a supported CI environment
    if !ambient::is_ci_environment() {
        return Err(miette::miette!(
            "Not running in a supported CI/CD environment.\n\
             Sigstore attestation requires one of:\n\
             - GitHub Actions (with 'id-token: write' permission)\n\
             - GitLab CI\n\
             - Google Cloud Build\n\
             - Buildkite\n\
             - CircleCI"
        ));
    }

    let detected = ambient::detect_environment();
    tracing::info!("Detected CI environment: {:?}", detected);

    ambient::get_ambient_token().await.map_err(|e| {
        miette::miette!(
            "Failed to get identity token from CI environment: {}\n\n\
             Troubleshooting:\n\
             1. For GitHub Actions, ensure you have 'id-token: write' permission\n\
             2. For GitLab CI, ensure CI_JOB_JWT_V2 or CI_JOB_JWT is available\n\
             3. Check that your CI workflow is correctly configured for OIDC",
            e
        )
    })
}

/// Store a signed attestation bundle to GitHub's attestation API
async fn store_attestation_to_github(
    bundle_json: &str,
    github_token: &str,
    owner: &str,
    repo: &str,
    client: &ClientWithMiddleware,
) -> miette::Result<String> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/attestations");

    // Parse the bundle JSON to ensure it's valid
    let bundle: serde_json::Value = serde_json::from_str(bundle_json)
        .into_diagnostic()
        .map_err(|e| miette::miette!("Invalid bundle JSON: {}", e))?;

    let request_body = json!({
        "bundle": bundle,
    });

    tracing::debug!("Storing attestation to GitHub at {}", url);

    let response = client
        .post(&url)
        .bearer_auth(github_token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&request_body)
        .send()
        .await
        .into_diagnostic()?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.into_diagnostic()?;
        let error_detail = match status.as_u16() {
            401 => "Authentication failed. Check your GitHub token.",
            403 => "Permission denied. Ensure token has 'attestations:write' and repository allows attestations.",
            404 => "Repository not found or attestations API unavailable. Ensure you're on a supported GitHub plan.",
            422 => "Invalid attestation bundle format.",
            _ => "Unknown error storing attestation.",
        };

        return Err(miette::miette!(
            "{}\nStatus: {}\nResponse: {}",
            error_detail,
            status,
            body
        ));
    }

    let response_data: AttestationResponse = response.json().await.into_diagnostic()?;
    tracing::info!(
        "Successfully stored attestation with ID: {}",
        response_data.id
    );

    Ok(response_data.id)
}
