//! Native Sigstore attestation creation for conda packages
//!
//! This module provides attestation creation using the sigstore-sign crate.

use base64::Engine;
use miette::{IntoDiagnostic, WrapErr};
use reqwest::header;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sigstore_sign::{Attestation, SigningContext, SigningServices, oidc::IdentityToken};
use sigstore_trust_root::SigningConfig as TufSigningConfig;
use std::path::Path;

pub use crate::upload::opt::SigstoreInstanceChoice;

/// Conda V1 predicate
#[derive(Debug, Serialize, Deserialize)]
pub struct CondaV1Predicate {
    #[serde(rename = "targetChannel", skip_serializing_if = "Option::is_none")]
    pub target_channel: Option<String>,
}

/// Response from GitHub attestation API
#[derive(Debug, Serialize, Deserialize)]
pub struct AttestationResponse {
    pub id: u64,
}

/// Configuration for attestation creation
#[derive(Debug, Clone, Default)]
pub struct AttestationConfig {
    pub repo_owner: Option<String>,
    pub repo_name: Option<String>,
    pub github_token: Option<String>,
    /// The Sigstore instance to sign with.
    pub sigstore_instance: SigstoreInstanceChoice,
}

/// The Sigstore instance an attestation is actually signed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedInstance {
    PublicGood,
    GitHub,
}

/// The claims of a GitHub Actions identity token that decide which instance
/// to sign with. They are read without verifying the token: Fulcio
/// authenticates it, and a forged claim could at worst pick the other
/// instance, whose Fulcio would then reject the token or record it publicly
/// exactly as the public-good instance always does.
#[derive(Debug, Deserialize)]
struct GitHubTokenClaims {
    iss: String,
    #[serde(default)]
    repository_visibility: Option<String>,
}

fn github_token_claims(token: &IdentityToken) -> Option<GitHubTokenClaims> {
    let payload = token.expose_secret().split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Returns true if `issuer` is a GitHub Actions issuer:
/// `https://token.actions.githubusercontent.com` for github.com,
/// `https://token.actions.<tenant>.ghe.com` for GitHub Enterprise Cloud tenants
/// with data residency, and either with an `/<enterprise>` path for
/// enterprises with a customized issuer.
fn is_github_actions_issuer(issuer: &str) -> bool {
    let Ok(url) = url::Url::parse(issuer) else {
        return false;
    };
    let plain = url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    let Some(host) = url.host_str().filter(|_| plain) else {
        return false;
    };
    host == "token.actions.githubusercontent.com"
        || host
            .strip_prefix("token.actions.")
            .and_then(|rest| rest.strip_suffix(".ghe.com"))
            .is_some_and(|tenant| {
                !tenant.is_empty()
                    && tenant
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
}

/// Decides which instance to sign with. `auto` mirrors `actions/attest`:
/// GitHub's instance for any GitHub repository that is not public, so that
/// the name of a private repository never ends up in the public Rekor log.
fn resolve_instance(
    choice: SigstoreInstanceChoice,
    token: &IdentityToken,
) -> miette::Result<ResolvedInstance> {
    let claims = github_token_claims(token);
    let is_github = claims
        .as_ref()
        .is_some_and(|claims| is_github_actions_issuer(&claims.iss));
    match choice {
        SigstoreInstanceChoice::PublicGood => Ok(ResolvedInstance::PublicGood),
        SigstoreInstanceChoice::Github if is_github => Ok(ResolvedInstance::GitHub),
        SigstoreInstanceChoice::Github => Err(miette::miette!(
            "GitHub's Sigstore instance only accepts GitHub Actions identity tokens, \
             but the ambient identity token was issued by {}",
            claims.map_or_else(|| "an unknown issuer".to_string(), |claims| claims.iss)
        )),
        // GitHub's instance only accepts GitHub Actions tokens.
        SigstoreInstanceChoice::Auto if !is_github => Ok(ResolvedInstance::PublicGood),
        SigstoreInstanceChoice::Auto => {
            match claims.and_then(|claims| claims.repository_visibility) {
                Some(visibility) if visibility == "public" => Ok(ResolvedInstance::PublicGood),
                // Private, internal, or a token that does not say: do not
                // risk publishing the repository name.
                _ => Ok(ResolvedInstance::GitHub),
            }
        }
    }
}

/// GitHub's signing services for the GitHub host this workflow runs on:
/// `fulcio.githubapp.com` for github.com and `fulcio.<tenant>.ghe.com` for
/// GitHub Enterprise Cloud tenants with data residency, like `actions/attest`.
fn github_signing_services() -> SigningServices {
    match std::env::var("GITHUB_SERVER_URL")
        .ok()
        .and_then(|server| url::Url::parse(&server).ok())
        .and_then(|server| server.host_str().map(str::to_owned))
    {
        Some(host) => SigningServices::github_for_host(&host),
        None => SigningServices::github(),
    }
}

/// Create and store an attestation for a conda package using native Sigstore signing
///
/// This function:
/// 1. Creates an in-toto statement for the package
/// 2. Signs the statement with the ambient OIDC identity, either with the
///    Sigstore public-good instance (Fulcio + Rekor) or, for private GitHub
///    repositories, with GitHub's instance (Fulcio + TSA, no public log); see
///    [`SigstoreInstanceChoice`]
/// 3. Optionally stores the signed attestation to GitHub's attestation API (if
///    token provided). Failing to store it only produces a warning.
///
/// Returns the attestation bundle JSON or GitHub attestation ID
pub async fn create_attestation(
    package_path: &Path,
    channel_url: &str,
    config: &AttestationConfig,
    client: &ClientWithMiddleware,
) -> miette::Result<String> {
    // Step 1: Get identity token from ambient environment (GitHub Actions, GitLab CI, etc.)
    let identity_token = IdentityToken::detect_ambient()
        .await
        .into_diagnostic()
        .with_context(|| "Failed to detect OIDC identity token")?
        .ok_or_else(|| miette::miette!("No OIDC identity token found in environment. Attestation signing is supported in GitHub Actions, GitLab CI, and other OIDC-enabled CI environments."))?;

    // Step 2: Compute package digest
    let digest = rattler_digest::compute_file_digest::<rattler_digest::Sha256>(package_path)
        .into_diagnostic()?;
    let digest_hex = hex::encode(digest);

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

    // Step 5: Pick the Sigstore instance and its signing services
    let signing_services = match resolve_instance(config.sigstore_instance, &identity_token)? {
        ResolvedInstance::GitHub => {
            let services = github_signing_services();
            tracing::info!(
                "Signing with GitHub's Sigstore instance ({}); the attestation is not recorded in a public transparency log",
                services.fulcio_url()
            );
            services
        }
        ResolvedInstance::PublicGood => {
            tracing::info!("Fetching Sigstore signing config via TUF...");
            let tuf_config = TufSigningConfig::production().await.map_err(|e| {
                miette::miette!("Failed to fetch Sigstore TUF signing config: {}", e)
            })?;
            let services = SigningServices::from_signing_config(&tuf_config)
                .map_err(|e| miette::miette!("Failed to build Sigstore signing config: {}", e))?;
            tracing::info!(
                "Signing with the Sigstore public-good instance; the signing identity is recorded in the public Rekor log"
            );
            services
        }
    };

    tracing::info!("Signing attestation with Sigstore...");
    let context = SigningContext::new(signing_services);
    let signer = context.signer(identity_token);

    let bundle = signer
        .sign_attestation(attestation)
        .await
        .map_err(|e| miette::miette!("Failed to sign attestation with Sigstore: {}", e))?;

    let bundle_json = bundle
        .to_json_pretty()
        .map_err(|e| miette::miette!("Failed to serialize bundle: {}", e))?;

    tracing::info!("Successfully created Sigstore attestation");

    // Step 6: Optionally store to GitHub if token is provided. The bundle is
    // what gets uploaded to the channel, so failing to also store it on GitHub
    // (for example because the plan does not support attestations for private
    // repositories) does not fail the attestation.
    if let (Some(token), Some(owner), Some(repo)) =
        (&config.github_token, &config.repo_owner, &config.repo_name)
    {
        match store_attestation_to_github(&bundle_json, token, owner, repo, client).await {
            Ok(attestation_id) => {
                tracing::info!("Attestation stored to GitHub with ID: {}", attestation_id);
            }
            Err(err) => {
                tracing::warn!("Failed to store the attestation to GitHub: {err:?}");
            }
        }
    }

    // Always return the bundle JSON for uploading to prefix.dev
    Ok(bundle_json)
}

/// Store a signed attestation bundle to GitHub's attestation API
async fn store_attestation_to_github(
    bundle_json: &str,
    github_token: &str,
    owner: &str,
    repo: &str,
    client: &ClientWithMiddleware,
) -> miette::Result<String> {
    // `GITHUB_API_URL` points at the tenant's API on GitHub Enterprise Cloud
    // with data residency (`https://api.<tenant>.ghe.com`).
    let api_url = std::env::var("GITHUB_API_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| "https://api.github.com".to_string());
    let url = format!(
        "{}/repos/{owner}/{repo}/attestations",
        api_url.trim_end_matches('/')
    );

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
            403 => {
                "Permission denied. Ensure token has 'attestations:write' and repository allows attestations."
            }
            404 => {
                "Repository not found or attestations API unavailable. Artifact attestations for private and internal repositories require GitHub Enterprise Cloud."
            }
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

    let body = response.text().await.into_diagnostic()?;
    let response_data: AttestationResponse = serde_json::from_str(&body)
        .map_err(|e| miette::miette!("Failed to parse GitHub response: {}\nBody: {}", e, body))?;
    tracing::info!(
        "Successfully stored attestation with ID: {}",
        response_data.id
    );

    Ok(response_data.id.to_string())
}

#[cfg(test)]
mod tests {
    const GITHUB_ACTIONS_ISSUER: &str = "https://token.actions.githubusercontent.com";

    use super::*;

    fn token(claims: serde_json::Value) -> IdentityToken {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap());
        IdentityToken::from_jwt(&format!("e30.{payload}.c2ln")).unwrap()
    }

    fn github_token(visibility: Option<&str>) -> IdentityToken {
        let mut claims = json!({
            "iss": GITHUB_ACTIONS_ISSUER,
            "sub": "repo:org/repo:ref:refs/heads/main",
            "exp": 9_999_999_999u64,
        });
        if let Some(visibility) = visibility {
            claims["repository_visibility"] = json!(visibility);
        }
        token(claims)
    }

    #[test]
    fn auto_signs_private_github_repositories_with_github() {
        let resolve = |token| resolve_instance(SigstoreInstanceChoice::Auto, &token).unwrap();
        assert_eq!(
            resolve(github_token(Some("public"))),
            ResolvedInstance::PublicGood
        );
        assert_eq!(
            resolve(github_token(Some("private"))),
            ResolvedInstance::GitHub
        );
        assert_eq!(
            resolve(github_token(Some("internal"))),
            ResolvedInstance::GitHub
        );
        // A GitHub token that does not state its visibility is not trusted to
        // be public.
        assert_eq!(resolve(github_token(None)), ResolvedInstance::GitHub);

        let gitlab = token(json!({
            "iss": "https://gitlab.com",
            "sub": "project_path:org/repo:ref_type:branch:ref:main",
            "exp": 9_999_999_999u64,
        }));
        assert_eq!(resolve(gitlab), ResolvedInstance::PublicGood);
    }

    #[test]
    fn explicit_instance_choice_wins() {
        assert_eq!(
            resolve_instance(
                SigstoreInstanceChoice::PublicGood,
                &github_token(Some("private"))
            )
            .unwrap(),
            ResolvedInstance::PublicGood
        );
        assert_eq!(
            resolve_instance(
                SigstoreInstanceChoice::Github,
                &github_token(Some("public"))
            )
            .unwrap(),
            ResolvedInstance::GitHub
        );
        let gitlab = token(json!({
            "iss": "https://gitlab.com",
            "sub": "x",
            "exp": 9_999_999_999u64,
        }));
        assert!(resolve_instance(SigstoreInstanceChoice::Github, &gitlab).is_err());
    }

    #[test]
    fn github_actions_issuer_matching() {
        assert!(is_github_actions_issuer(GITHUB_ACTIONS_ISSUER));
        assert!(is_github_actions_issuer(
            "https://token.actions.githubusercontent.com/octocorp"
        ));
        assert!(is_github_actions_issuer(
            "https://token.actions.octocorp.ghe.com"
        ));
        assert!(!is_github_actions_issuer(
            "https://token.actions.githubusercontent.com.evil.example"
        ));
        assert!(!is_github_actions_issuer(
            "https://token.actions.octocorp.ghe.com.evil.example"
        ));
        assert!(!is_github_actions_issuer(
            "http://token.actions.githubusercontent.com"
        ));
        assert!(!is_github_actions_issuer("https://gitlab.com"));
    }
}
