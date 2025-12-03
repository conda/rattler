// This code has been adapted from uv under https://github.com/astral-sh/uv/blob/c5caf92edf539a9ebf24d375871178f8f8a0ab93/crates/uv-publish/src/trusted_publishing.rs
// The original code is dual-licensed under Apache-2.0 and MIT

//! Trusted publishing (via OIDC) with GitHub Actions, GitLab CI, and Google Cloud.

use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::env;
use std::env::VarError;
use std::ffi::OsString;
use thiserror::Error;
use url::Url;

use crate::utils::console_utils::{github_action_runner, gitlab_ci_runner, google_cloud_runner};
use crate::utils::consts;

/// Represents the CI provider being used for trusted publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CIProvider {
    GitHubActions,
    GitLabCI,
    GoogleCloud,
}

impl std::fmt::Display for CIProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CIProvider::GitHubActions => write!(f, "GitHub Actions"),
            CIProvider::GitLabCI => write!(f, "GitLab CI"),
            CIProvider::GoogleCloud => write!(f, "Google Cloud"),
        }
    }
}

/// Detects which CI provider is being used, if any.
pub fn detect_ci_provider() -> Option<CIProvider> {
    if github_action_runner() {
        Some(CIProvider::GitHubActions)
    } else if gitlab_ci_runner() {
        Some(CIProvider::GitLabCI)
    } else if google_cloud_runner() {
        Some(CIProvider::GoogleCloud)
    } else {
        None
    }
}

/// If applicable, attempt obtaining a token for trusted publishing.
pub async fn check_trusted_publishing(
    client: &ClientWithMiddleware,
    prefix_url: &Url,
) -> TrustedPublishResult {
    // Check which CI provider we're running on
    let provider = match detect_ci_provider() {
        Some(p) => p,
        None => return TrustedPublishResult::Skipped,
    };

    tracing::debug!(
        "Running on {} without explicit credentials, checking for trusted publishing",
        provider
    );

    match get_token(client, prefix_url, provider).await {
        Ok(token) => TrustedPublishResult::Configured(token),
        Err(err) => {
            tracing::debug!("Could not obtain trusted publishing credentials, skipping: {err}");
            TrustedPublishResult::Ignored(err)
        }
    }
}

pub enum TrustedPublishResult {
    /// We didn't check for trusted publishing.
    Skipped,
    /// We checked for trusted publishing and found a token.
    Configured(TrustedPublishingToken),
    /// We checked for optional trusted publishing, but it didn't succeed.
    Ignored(TrustedPublishingError),
}

#[derive(Debug, Error)]
pub enum TrustedPublishingError {
    #[error("Environment variable {0} not set, is the `id-token: write` permission missing?")]
    MissingEnvVar(&'static str),
    #[error("Environment variable {0} is not valid UTF-8: `{1:?}`")]
    InvalidEnvVar(&'static str, OsString),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error("Failed to fetch: `{0}`")]
    Reqwest(Url, #[source] reqwest::Error),
    #[error("Failed to fetch: `{0}`")]
    ReqwestMiddleware(Url, #[source] reqwest_middleware::Error),
    #[error(
        "Prefix.dev returned error code {0}, is trusted publishing correctly configured?\nResponse: {1}"
    )]
    PrefixDev(StatusCode, String),
    #[error("GitLab CI OIDC token not found. Make sure you have configured `id_tokens` in your .gitlab-ci.yml:\n\n\
        job_name:\n  \
          id_tokens:\n    \
            PREFIX_ID_TOKEN:\n      \
              aud: prefix.dev\n")]
    GitLabOidcTokenNotFound,
    #[error("Google Cloud OIDC token retrieval failed. Make sure you are running in a Google Cloud environment (Cloud Build, Cloud Run, GCE, or GKE) with a service account attached.")]
    GoogleCloudOidcTokenNotFound,
}

impl TrustedPublishingError {
    fn from_var_err(env_var: &'static str, err: VarError) -> Self {
        match err {
            VarError::NotPresent => Self::MissingEnvVar(env_var),
            VarError::NotUnicode(os_string) => Self::InvalidEnvVar(env_var, os_string),
        }
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
pub struct TrustedPublishingToken(String);

impl TrustedPublishingToken {
    pub fn secret(&self) -> &str {
        &self.0
    }
}

/// The response from querying `$ACTIONS_ID_TOKEN_REQUEST_URL&audience=prefix.dev`.
#[derive(Deserialize)]
struct OidcToken {
    value: String,
}

/// The body for querying `$ACTIONS_ID_TOKEN_REQUEST_URL&audience=prefix.dev`.
#[derive(Serialize)]
struct MintTokenRequest {
    token: String,
}

/// Returns the short-lived token to use for uploading.
pub async fn get_token(
    client: &ClientWithMiddleware,
    prefix_url: &Url,
    provider: CIProvider,
) -> Result<TrustedPublishingToken, TrustedPublishingError> {
    // Get the OIDC token based on the CI provider
    let oidc_token = match provider {
        CIProvider::GitHubActions => {
            // If this fails, we can skip the audience request.
            let oidc_token_request_token = env::var(consts::ACTIONS_ID_TOKEN_REQUEST_TOKEN)
                .map_err(|err| {
                    TrustedPublishingError::from_var_err(
                        consts::ACTIONS_ID_TOKEN_REQUEST_TOKEN,
                        err,
                    )
                })?;

            // Request 1: Get the OIDC token from GitHub.
            get_github_oidc_token(&oidc_token_request_token, client).await?
        }
        CIProvider::GitLabCI => {
            // Get the OIDC token from GitLab CI environment variable
            get_gitlab_oidc_token()?
        }
        CIProvider::GoogleCloud => {
            // Get the OIDC token from Google Cloud metadata server
            get_google_cloud_oidc_token(client).await?
        }
    };

    // Request 2: Get the publishing token from prefix.dev.
    let publish_token = get_publish_token(&oidc_token, prefix_url, client).await?;

    tracing::info!("Received token from {}, using trusted publishing", provider);

    // Mask the token in CI logs
    match provider {
        CIProvider::GitHubActions => {
            println!("::add-mask::{}", &publish_token.secret());
        }
        CIProvider::GitLabCI => {
            // GitLab CI doesn't have a built-in mask mechanism like GitHub Actions,
            // but the token should be short-lived anyway
            tracing::debug!("Token obtained via GitLab CI trusted publishing");
        }
        CIProvider::GoogleCloud => {
            // Google Cloud doesn't have a built-in mask mechanism,
            // but the token should be short-lived anyway
            tracing::debug!("Token obtained via Google Cloud trusted publishing");
        }
    }

    Ok(publish_token)
}

/// Get raw OIDC token for attestation generation.
/// Returns the OIDC token from the current CI provider.
pub async fn get_raw_oidc_token(
    client: &ClientWithMiddleware,
) -> Result<String, TrustedPublishingError> {
    match detect_ci_provider() {
        Some(CIProvider::GitHubActions) => {
            let oidc_token_request_token = env::var(consts::ACTIONS_ID_TOKEN_REQUEST_TOKEN)
                .map_err(|err| {
                    TrustedPublishingError::from_var_err(
                        consts::ACTIONS_ID_TOKEN_REQUEST_TOKEN,
                        err,
                    )
                })?;
            get_github_oidc_token(&oidc_token_request_token, client).await
        }
        Some(CIProvider::GitLabCI) => get_gitlab_oidc_token(),
        Some(CIProvider::GoogleCloud) => get_google_cloud_oidc_token(client).await,
        None => Err(TrustedPublishingError::MissingEnvVar(
            "GITHUB_ACTIONS, GITLAB_CI, or CLOUD_BUILD_ID/K_SERVICE",
        )),
    }
}

/// Get the OIDC token from GitLab CI.
/// GitLab CI provides the token via the `PREFIX_ID_TOKEN` environment variable
/// when configured with `id_tokens` in the `.gitlab-ci.yml` file.
fn get_gitlab_oidc_token() -> Result<String, TrustedPublishingError> {
    // GitLab CI provides OIDC tokens via the `id_tokens` keyword in .gitlab-ci.yml
    // The user should configure their job like this:
    //
    // job_name:
    //   id_tokens:
    //     PREFIX_ID_TOKEN:
    //       aud: prefix.dev
    //   script:
    //     - rattler upload ...
    //
    // The token is then available as the PREFIX_ID_TOKEN environment variable.
    match env::var(consts::PREFIX_ID_TOKEN) {
        Ok(token) if !token.is_empty() => {
            tracing::info!("Found GitLab CI OIDC token in PREFIX_ID_TOKEN");
            Ok(token)
        }
        Ok(_) => {
            tracing::warn!("PREFIX_ID_TOKEN is set but empty");
            Err(TrustedPublishingError::GitLabOidcTokenNotFound)
        }
        Err(_) => {
            tracing::debug!("PREFIX_ID_TOKEN not found in environment");
            Err(TrustedPublishingError::GitLabOidcTokenNotFound)
        }
    }
}

/// Get the OIDC token from GitHub Actions.
async fn get_github_oidc_token(
    oidc_token_request_token: &str,
    client: &ClientWithMiddleware,
) -> Result<String, TrustedPublishingError> {
    let oidc_token_url = env::var(consts::ACTIONS_ID_TOKEN_REQUEST_URL).map_err(|err| {
        TrustedPublishingError::from_var_err(consts::ACTIONS_ID_TOKEN_REQUEST_URL, err)
    })?;
    let mut oidc_token_url = Url::parse(&oidc_token_url)?;
    oidc_token_url
        .query_pairs_mut()
        .append_pair("audience", "prefix.dev");
    tracing::info!("Querying the trusted publishing OIDC token from {oidc_token_url}");
    let response = client
        .get(oidc_token_url.clone())
        .bearer_auth(oidc_token_request_token)
        .send()
        .await
        .map_err(|err| TrustedPublishingError::ReqwestMiddleware(oidc_token_url.clone(), err))?;
    let oidc_token: OidcToken = response
        .error_for_status()
        .map_err(|err| TrustedPublishingError::Reqwest(oidc_token_url.clone(), err))?
        .json()
        .await
        .map_err(|err| TrustedPublishingError::Reqwest(oidc_token_url.clone(), err))?;
    Ok(oidc_token.value)
}

/// Get the OIDC token from Google Cloud metadata server.
/// Works in Cloud Build, Cloud Run, GCE, and GKE with Workload Identity.
async fn get_google_cloud_oidc_token(
    client: &ClientWithMiddleware,
) -> Result<String, TrustedPublishingError> {
    let metadata_url = format!("{}?audience=prefix.dev", consts::GCP_METADATA_IDENTITY_URL);
    let url = Url::parse(&metadata_url)?;

    tracing::info!("Querying the trusted publishing OIDC token from Google Cloud metadata server");

    let response = client
        .get(url.clone())
        .header("Metadata-Flavor", "Google")
        .send()
        .await
        .map_err(|err| TrustedPublishingError::ReqwestMiddleware(url.clone(), err))?;

    if !response.status().is_success() {
        tracing::warn!(
            "Google Cloud metadata server returned status {}",
            response.status()
        );
        return Err(TrustedPublishingError::GoogleCloudOidcTokenNotFound);
    }

    let token = response
        .text()
        .await
        .map_err(|err| TrustedPublishingError::Reqwest(url.clone(), err))?;

    if token.is_empty() {
        tracing::warn!("Google Cloud metadata server returned empty token");
        return Err(TrustedPublishingError::GoogleCloudOidcTokenNotFound);
    }

    tracing::info!("Successfully obtained OIDC token from Google Cloud");
    Ok(token)
}

async fn get_publish_token(
    oidc_token: &str,
    prefix_url: &Url,
    client: &ClientWithMiddleware,
) -> Result<TrustedPublishingToken, TrustedPublishingError> {
    let mint_token_url = prefix_url.join("/api/oidc/mint_token")?;
    tracing::info!("Querying the trusted publishing upload token from {mint_token_url}");
    let mint_token_payload = MintTokenRequest {
        token: oidc_token.to_string(),
    };

    let response = client
        .post(mint_token_url.clone())
        .json(&mint_token_payload)
        .send()
        .await
        .map_err(|err| TrustedPublishingError::ReqwestMiddleware(mint_token_url.clone(), err))?;

    // reqwest's implementation of `.json()` also goes through `.bytes()`
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|err| TrustedPublishingError::Reqwest(mint_token_url.clone(), err))?;

    if status.is_success() {
        let token = TrustedPublishingToken(String::from_utf8_lossy(&body).to_string());
        Ok(token)
    } else {
        // An error here means that something is misconfigured,
        // so we're showing the body for more context
        Err(TrustedPublishingError::PrefixDev(
            status,
            String::from_utf8_lossy(&body).to_string(),
        ))
    }
}
