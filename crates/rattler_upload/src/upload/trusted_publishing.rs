// This code has been adapted from uv under https://github.com/astral-sh/uv/blob/c5caf92edf539a9ebf24d375871178f8f8a0ab93/crates/uv-publish/src/trusted_publishing.rs
// The original code is dual-licensed under Apache-2.0 and MIT

//! Trusted publishing (via OIDC) with GitHub actions.

use reqwest::{StatusCode, header};
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::env;
use std::env::VarError;
use std::ffi::OsString;
use thiserror::Error;
use url::Url;

use crate::{console_utils::github_action_runner, consts};

/// If applicable, attempt obtaining a token for trusted publishing.
pub async fn check_trusted_publishing(
    client: &ClientWithMiddleware,
    prefix_url: &Url,
) -> TrustedPublishResult {
    // If we aren't in GitHub Actions, we can't use trusted publishing.
    if !github_action_runner() {
        return TrustedPublishResult::Skipped;
    }
    // We could check for credentials from the keyring or netrc the auth middleware first, but
    // given that we are in GitHub Actions we check for trusted publishing first.
    tracing::debug!(
        "Running on GitHub Actions without explicit credentials, checking for trusted publishing"
    );
    match get_token(client, prefix_url).await {
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
pub(crate) async fn get_token(
    client: &ClientWithMiddleware,
    prefix_url: &Url,
) -> Result<TrustedPublishingToken, TrustedPublishingError> {
    // If this fails, we can skip the audience request.
    let oidc_token_request_token =
        env::var(consts::ACTIONS_ID_TOKEN_REQUEST_TOKEN).map_err(|err| {
            TrustedPublishingError::from_var_err(consts::ACTIONS_ID_TOKEN_REQUEST_TOKEN, err)
        })?;

    // Request 1: Get the OIDC token from GitHub.
    let oidc_token = get_oidc_token(&oidc_token_request_token, client).await?;

    // Request 2: Get the publishing token from prefix.dev.
    let publish_token = get_publish_token(&oidc_token, prefix_url, client).await?;

    tracing::info!("Received token, using trusted publishing");

    // Tell GitHub Actions to mask the token in any console logs.
    if github_action_runner() {
        println!("::add-mask::{}", &publish_token.secret());
    }

    Ok(publish_token)
}

async fn get_oidc_token(
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
    let authorization = format!("bearer {oidc_token_request_token}");
    let response = client
        .get(oidc_token_url.clone())
        .header(header::AUTHORIZATION, authorization)
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
