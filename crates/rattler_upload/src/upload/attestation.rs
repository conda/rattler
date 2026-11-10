//! Native Sigstore attestation creation for conda packages
//!
//! This module provides attestation creation using the sigstore-sign crate.

use base64::Engine;
use miette::{IntoDiagnostic, WrapErr};
use rattler_sigstore::GitHubActionsIssuer;
use reqwest::header;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sigstore_sign::{Attestation, SigningContext, SigningServices, oidc::IdentityToken};
use sigstore_trust_root::{SigningConfig as TufSigningConfig, SigstoreInstance};
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
#[non_exhaustive]
pub struct AttestationConfig {
    pub repo_owner: Option<String>,
    pub repo_name: Option<String>,
    pub github_token: Option<String>,
    /// The Sigstore instance to sign with.
    pub sigstore_instance: SigstoreInstanceChoice,
}

impl AttestationConfig {
    /// Also stores the attestation to the attestations API of the GitHub
    /// repository `owner/name`, authenticating with `token`.
    pub fn with_github_repository(
        mut self,
        owner: impl Into<String>,
        name: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        self.repo_owner = Some(owner.into());
        self.repo_name = Some(name.into());
        self.github_token = Some(token.into());
        self
    }

    /// Sets the Sigstore instance to sign with.
    pub fn with_sigstore_instance(mut self, sigstore_instance: SigstoreInstanceChoice) -> Self {
        self.sigstore_instance = sigstore_instance;
        self
    }
}

/// The Sigstore instance an attestation is actually signed with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedInstance {
    PublicGood,
    /// The GitHub instance of the GitHub that issued the identity token.
    GitHub(GitHubActionsIssuer),
}

/// The claims of a GitHub Actions identity token that decide which instance
/// to sign with. They are read without verifying the token: Fulcio
/// authenticates it, and a forged claim could at worst pick another
/// instance, whose Fulcio would then reject the token or record it publicly
/// exactly as the public-good instance always does. The token is only ever
/// sent to the public-good instance or to a GitHub host derived from its
/// issuer (see [`github_signing_services`]).
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

/// Decides which instance to sign with. `auto` mirrors `actions/attest`:
/// GitHub's instance for any GitHub repository that is not public, so that
/// the name of a private repository never ends up in the public Rekor log.
fn resolve_instance(
    choice: SigstoreInstanceChoice,
    token: &IdentityToken,
) -> miette::Result<ResolvedInstance> {
    let claims = github_token_claims(token);
    let github = claims
        .as_ref()
        .and_then(|claims| GitHubActionsIssuer::parse(&claims.iss));
    match (choice, github) {
        // GitHub's instance only accepts GitHub Actions tokens.
        (SigstoreInstanceChoice::PublicGood, _) | (SigstoreInstanceChoice::Auto, None) => {
            Ok(ResolvedInstance::PublicGood)
        }
        (SigstoreInstanceChoice::Github, Some(github)) => Ok(ResolvedInstance::GitHub(github)),
        (SigstoreInstanceChoice::Github, None) => Err(miette::miette!(
            "GitHub's Sigstore instance only accepts GitHub Actions identity tokens, \
             but the ambient identity token was issued by {}",
            claims.map_or_else(|| "an unknown issuer".to_string(), |claims| claims.iss)
        )),
        (SigstoreInstanceChoice::Auto, Some(github)) => {
            match claims.and_then(|claims| claims.repository_visibility) {
                Some(visibility) if visibility == "public" => Ok(ResolvedInstance::PublicGood),
                // Private, internal, or a token that does not say: do not
                // risk publishing the repository name.
                _ => Ok(ResolvedInstance::GitHub(github)),
            }
        }
    }
}

/// GitHub's signing services for the GitHub that issued the identity token,
/// like `actions/attest`: the embedded signing config of GitHub's instance for
/// github.com, and `fulcio.<tenant>.ghe.com` / `timestamp.<tenant>.ghe.com`
/// for GitHub Enterprise Cloud tenants with data residency. None of them runs
/// a transparency log.
///
/// The hosts are derived from the token's issuer rather than from the
/// environment (`GITHUB_SERVER_URL`), so that the token is only ever sent to
/// the GitHub it was issued by: github.com's instance, or a `*.ghe.com` host
/// of the tenant named in the issuer.
fn github_signing_services(issuer: &GitHubActionsIssuer) -> miette::Result<SigningServices> {
    match issuer {
        GitHubActionsIssuer::GitHubCom => SigningServices::from_embedded(SigstoreInstance::GitHub)
            .map_err(|e| miette::miette!("Failed to load GitHub's signing config: {}", e)),
        GitHubActionsIssuer::Tenant(tenant) => Ok(SigningServices::without_transparency_log(
            format!("https://fulcio.{tenant}.ghe.com"),
            format!("https://timestamp.{tenant}.ghe.com/api/v1/timestamp"),
        )),
    }
}

/// Why an attestation could not be stored to GitHub's attestations API.
#[derive(Debug, thiserror::Error)]
enum StoreAttestationError {
    /// GitHub does not offer artifact attestations for this repository: they
    /// need a public repository or GitHub Enterprise Cloud.
    #[error(
        "GitHub does not offer artifact attestations for this repository; they require a public \
         repository or GitHub Enterprise Cloud (status {status}): {body}"
    )]
    FeatureNotAvailable {
        status: reqwest::StatusCode,
        body: String,
    },
    /// Any other failure.
    #[error("{0}")]
    Other(String),
}

/// Whether a failed response of GitHub's attestations API says that the
/// repository's plan does not include artifact attestations. `actions/attest`
/// reports this as "Feature not available for ...", which is matched here
/// because the status code alone does not identify it.
fn is_feature_not_available(body: &str) -> bool {
    body.to_ascii_lowercase().contains("feature not available")
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
///    token provided). Failing to store it is an error, except when GitHub
///    does not offer artifact attestations for the repository, which only
///    produces a warning.
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
        ResolvedInstance::GitHub(issuer) => {
            let services = github_signing_services(&issuer)?;
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
    // what gets uploaded to the channel, so a repository whose plan does not
    // include artifact attestations (private repositories outside GitHub
    // Enterprise Cloud) can still be attested. Every other failure, such as a
    // token without `attestations: write`, fails the attestation: the user
    // asked for it to be stored.
    if let (Some(token), Some(owner), Some(repo)) =
        (&config.github_token, &config.repo_owner, &config.repo_name)
    {
        // `GITHUB_API_URL` points at the tenant's API on GitHub Enterprise
        // Cloud with data residency (`https://api.<tenant>.ghe.com`).
        let api_url = std::env::var("GITHUB_API_URL")
            .ok()
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| "https://api.github.com".to_string());
        match store_attestation_to_github(&api_url, &bundle_json, token, owner, repo, client).await
        {
            Ok(attestation_id) => {
                tracing::info!("Attestation stored to GitHub with ID: {}", attestation_id);
            }
            Err(err @ StoreAttestationError::FeatureNotAvailable { .. }) => {
                tracing::warn!("The attestation was not stored to GitHub: {err}");
            }
            Err(err) => {
                return Err(miette::miette!(
                    "Failed to store the attestation to GitHub: {err}"
                ));
            }
        }
    }

    // Always return the bundle JSON for uploading to prefix.dev
    Ok(bundle_json)
}

/// Store a signed attestation bundle to GitHub's attestation API
async fn store_attestation_to_github(
    api_url: &str,
    bundle_json: &str,
    github_token: &str,
    owner: &str,
    repo: &str,
    client: &ClientWithMiddleware,
) -> Result<String, StoreAttestationError> {
    let other = |message: String| StoreAttestationError::Other(message);

    let url = format!(
        "{}/repos/{owner}/{repo}/attestations",
        api_url.trim_end_matches('/')
    );

    // Parse the bundle JSON to ensure it's valid
    let bundle: serde_json::Value = serde_json::from_str(bundle_json)
        .map_err(|e| other(format!("Invalid bundle JSON: {e}")))?;

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
        .map_err(|e| other(format!("Request to {url} failed: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        if is_feature_not_available(&body) {
            return Err(StoreAttestationError::FeatureNotAvailable { status, body });
        }
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

        return Err(other(format!(
            "{error_detail}\nStatus: {status}\nResponse: {body}"
        )));
    }

    let body = response
        .text()
        .await
        .map_err(|e| other(format!("Failed to read GitHub response: {e}")))?;
    let response_data: AttestationResponse = serde_json::from_str(&body).map_err(|e| {
        other(format!(
            "Failed to parse GitHub response: {e}\nBody: {body}"
        ))
    })?;
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
            ResolvedInstance::GitHub(GitHubActionsIssuer::GitHubCom)
        );
        assert_eq!(
            resolve(github_token(Some("internal"))),
            ResolvedInstance::GitHub(GitHubActionsIssuer::GitHubCom)
        );
        // A GitHub token that does not state its visibility is not trusted to
        // be public.
        assert_eq!(
            resolve(github_token(None)),
            ResolvedInstance::GitHub(GitHubActionsIssuer::GitHubCom)
        );

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
            ResolvedInstance::GitHub(GitHubActionsIssuer::GitHubCom)
        );
        let gitlab = token(json!({
            "iss": "https://gitlab.com",
            "sub": "x",
            "exp": 9_999_999_999u64,
        }));
        assert!(resolve_instance(SigstoreInstanceChoice::Github, &gitlab).is_err());
    }

    #[test]
    fn github_signing_services_follow_the_token_issuer() {
        let services = github_signing_services(&GitHubActionsIssuer::GitHubCom).unwrap();
        assert_eq!(services.fulcio_url(), "https://fulcio.githubapp.com");
        assert_eq!(
            services.tsa_url(),
            Some("https://timestamp.githubapp.com/api/v1/timestamp")
        );
        assert_eq!(services.rekor_url(), None);

        let tenant =
            github_signing_services(&GitHubActionsIssuer::Tenant("octocorp".to_string())).unwrap();
        assert_eq!(tenant.fulcio_url(), "https://fulcio.octocorp.ghe.com");
        assert_eq!(
            tenant.tsa_url(),
            Some("https://timestamp.octocorp.ghe.com/api/v1/timestamp")
        );
        assert_eq!(tenant.rekor_url(), None);
    }

    #[test]
    fn tenant_tokens_sign_with_the_instance_of_their_tenant() {
        let tenant = token(json!({
            "iss": "https://token.actions.octocorp.ghe.com",
            "sub": "repo:org/repo:ref:refs/heads/main",
            "exp": 9_999_999_999u64,
            "repository_visibility": "private",
        }));
        // The signing hosts come from the token's issuer, not from the
        // environment.
        let resolved = resolve_instance(SigstoreInstanceChoice::Auto, &tenant).unwrap();
        assert_eq!(
            resolved,
            ResolvedInstance::GitHub(GitHubActionsIssuer::Tenant("octocorp".to_string()))
        );
        let ResolvedInstance::GitHub(issuer) = resolved else {
            unreachable!()
        };
        assert_eq!(
            github_signing_services(&issuer).unwrap().fulcio_url(),
            "https://fulcio.octocorp.ghe.com"
        );

        // A look-alike issuer is not a GitHub issuer, so its token is never
        // sent to a host derived from it.
        let look_alike = token(json!({
            "iss": "https://token.actions.octocorp.ghe.com.evil.example",
            "sub": "x",
            "exp": 9_999_999_999u64,
            "repository_visibility": "private",
        }));
        assert_eq!(
            resolve_instance(SigstoreInstanceChoice::Auto, &look_alike).unwrap(),
            ResolvedInstance::PublicGood
        );
        assert!(resolve_instance(SigstoreInstanceChoice::Github, &look_alike).is_err());
    }

    /// Serves `status` and `body` for every request and returns the base URL.
    async fn serve(status: u16, body: &'static str) -> String {
        let app = axum::Router::new().fallback(move || async move {
            (axum::http::StatusCode::from_u16(status).unwrap(), body)
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn only_a_missing_plan_feature_is_not_a_storage_failure() {
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        let store = |api_url: String| {
            let client = client.clone();
            async move {
                store_attestation_to_github(&api_url, "{}", "token", "org", "repo", &client).await
            }
        };

        let unavailable = serve(
            403,
            r#"{"message":"Feature not available for the org organization."}"#,
        )
        .await;
        assert!(matches!(
            store(unavailable).await,
            Err(StoreAttestationError::FeatureNotAvailable { .. })
        ));

        // A token without `attestations: write`, a missing repository and any
        // other failure stay errors.
        for (status, body) in [
            (
                403,
                r#"{"message":"Resource not accessible by integration"}"#,
            ),
            (404, r#"{"message":"Not Found"}"#),
            (401, r#"{"message":"Bad credentials"}"#),
        ] {
            let result = store(serve(status, body).await).await;
            assert!(
                matches!(result, Err(StoreAttestationError::Other(_))),
                "{status}: {result:?}"
            );
        }

        let stored = serve(201, r#"{"id": 42}"#).await;
        assert_eq!(store(stored).await.unwrap(), "42");
    }

    #[test]
    fn only_a_missing_plan_feature_is_recognized() {
        assert!(is_feature_not_available(
            r#"{"message":"Feature not available for user-owned private repositories. To enable this feature, please make this repository public."}"#
        ));
        assert!(!is_feature_not_available(
            r#"{"message":"Resource not accessible by integration"}"#
        ));
        assert!(!is_feature_not_available(r#"{"message":"Not Found"}"#));
    }
}
