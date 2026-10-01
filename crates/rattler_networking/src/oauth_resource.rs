//! Audience-specific OAuth credentials, separate from channel authentication.
//! For providers supporting the `audience` parameter (such as prefix.dev/Hydra),
//! not the separate RFC 8707 `resource` parameter.
//!
//! The exact issuer/client/audience tuple defines a non-host storage namespace.
//! This module never consults host/wildcard credentials. Access tokens are opaque:
//! the resource server verifies their audience/signature. Expiry must be supplied
//! by the token endpoint (`expires_in`) or a trusted acquisition callback.

use crate::{Authentication, AuthenticationStorage};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

/// A trusted resource authorization target. Do not construct from arbitrary
/// server challenges or audit-mirror URLs without an explicit trust decision.
#[derive(Clone, Debug)]
pub struct OAuthResource {
    issuer: String,
    client_id: String,
    audience: String,
}

/// Whether interactive authorization is permitted. Callers must deny in CI,
/// noninteractive execution, and offline mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceInteraction {
    /// Interactive browser/device authorization is allowed.
    Allow,
    /// Missing or unusable credentials produce an actionable error.
    Deny,
}

/// Secret-free errors. Provider response bodies and credential backend errors
/// are deliberately not embedded in user-facing diagnostics.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ResourceOAuthError {
    /// Invalid trusted configuration.
    #[error(
        "Invalid resource OAuth configuration (public client, HTTPS issuer, exact nonempty audience required)"
    )]
    InvalidConfiguration,
    /// A credential is not usable for this issuer/client/audience tuple.
    #[error(
        "Resource OAuth credential has invalid or mismatched metadata; credentials are unchanged"
    )]
    InvalidCredential,
    /// Storage cannot safely be read or written.
    #[error("Resource OAuth credential storage failed; no usable grant was returned")]
    Storage,
    /// No suitable credential exists and interaction is forbidden.
    #[error(
        "Resource authorization required; run interactively first, or provision audience-specific credentials for CI"
    )]
    AuthorizationRequired,
    /// Interactive authorization failed or was cancelled.
    #[error("Resource authorization did not complete; stored credentials are unchanged")]
    AuthorizationFailed,
    /// Refresh failed without a definitive `invalid_grant` response.
    #[error("Resource token refresh failed; retry later (stored credentials are unchanged)")]
    RefreshFailed,
}

/// A resource access token. Debug output never exposes access/refresh tokens.
/// Send only to the explicitly trusted resource endpoint, without redirects.
pub struct ResourceToken(Authentication);
impl fmt::Debug for ResourceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResourceToken([REDACTED])")
    }
}
impl ResourceToken {
    /// Secret bearer value. Do not log it or include it in command arguments.
    pub fn access_token(&self) -> &str {
        match &self.0 {
            Authentication::OAuth { access_token, .. } => access_token,
            _ => unreachable!(),
        }
    }
}

impl OAuthResource {
    /// Create a tuple without URL/audience normalization. HTTP is allowed only
    /// for literal loopback issuers used by local tests.
    pub fn new(
        issuer: String,
        client_id: String,
        audience: String,
    ) -> Result<Self, ResourceOAuthError> {
        let url = Url::parse(&issuer).map_err(|_error| ResourceOAuthError::InvalidConfiguration)?;
        if issuer.len() > 2048
            || !secure_url(&url)
            || url.query().is_some()
            || url.fragment().is_some()
            || client_id.trim().is_empty()
            || client_id.len() > 512
            || client_id.chars().any(char::is_control)
            || audience.is_empty()
            || audience.len() > 2048
            || audience
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(ResourceOAuthError::InvalidConfiguration);
        }
        Ok(Self {
            issuer,
            client_id,
            audience,
        })
    }
    /// Exact expected issuer.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    /// OAuth public client ID.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }
    /// Exact requested audience (not an OAuth scope).
    pub fn audience(&self) -> &str {
        &self.audience
    }
    /// Stable non-host key. Length-prefix ambiguity is avoided by JSON encoding.
    /// No host/wildcard lookup can select this key for a channel request.
    pub fn storage_key(&self) -> String {
        let tuple = serde_json::to_vec(&[&self.issuer, &self.client_id, &self.audience])
            .expect("string tuple serializes");
        format!(
            "oauth-resource-v1:{}",
            URL_SAFE_NO_PAD.encode(Sha256::digest(tuple))
        )
    }

    /// Reuse, refresh, or authorize an isolated resource grant. The supplied
    /// login function must request this audience from the trusted issuer.
    /// Offline never performs network I/O or invokes login; Deny still permits
    /// silent refresh. Clones of one storage share a per-tuple gate, including
    /// interactive acquisition. Separately constructed storage/backend instances
    /// and separate processes are not coordinated; share one storage across tasks.
    /// The callback must provide trusted expiry metadata, not unverified JWT claims.
    ///
    /// Credentials are returned only after successful persistence. A failed
    /// write preserves the local record, but cannot undo provider-side refresh
    /// token rotation; reauthorization can be necessary after a storage failure.
    pub async fn acquire<F, Fut>(
        &self,
        storage: &AuthenticationStorage,
        interaction: ResourceInteraction,
        offline: bool,
        login: F,
    ) -> Result<ResourceToken, ResourceOAuthError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Authentication, ResourceOAuthError>>,
    {
        self.acquire_inner(storage, interaction, offline, false, login)
            .await
    }

    /// Refresh after the resource rejects an otherwise unexpired cached token. Never
    /// opens a browser; requires a previously stored resource refresh grant.
    pub async fn refresh_cached(
        &self,
        storage: &AuthenticationStorage,
    ) -> Result<ResourceToken, ResourceOAuthError> {
        self.acquire_inner(storage, ResourceInteraction::Deny, false, true, || async {
            Err(ResourceOAuthError::AuthorizationRequired)
        })
        .await
    }

    async fn acquire_inner<F, Fut>(
        &self,
        storage: &AuthenticationStorage,
        interaction: ResourceInteraction,
        offline: bool,
        force_refresh: bool,
        login: F,
    ) -> Result<ResourceToken, ResourceOAuthError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Authentication, ResourceOAuthError>>,
    {
        let key = self.storage_key();
        let gate = storage.oauth_refresh_lock(&key);
        let _guard = gate.lock().await;
        if let Some(auth) = storage
            .read_resource(&key)
            .map_err(|_error| ResourceOAuthError::Storage)?
        {
            let expiry = self.validate(&auth)?;
            if !force_refresh && expiry > now().saturating_add(30) {
                return Ok(ResourceToken(auth));
            }
            if offline {
                return Err(ResourceOAuthError::AuthorizationRequired);
            }
            match self.refresh(&auth).await {
                Ok(Some(updated)) => {
                    if self.validate(&updated)? <= now().saturating_add(30) {
                        return Err(ResourceOAuthError::InvalidCredential);
                    }
                    storage
                        .write_resource(&key, &updated)
                        .map_err(|_error| ResourceOAuthError::Storage)?;
                    return Ok(ResourceToken(updated));
                }
                Ok(None) => {} // Missing/revoked refresh token: authorize if allowed.
                Err(error) => return Err(error), // Never turn transient outages into login prompts.
            }
        }
        if offline || interaction == ResourceInteraction::Deny {
            return Err(ResourceOAuthError::AuthorizationRequired);
        }
        let updated = login().await?;
        if self.validate(&updated)? <= now().saturating_add(30) {
            return Err(ResourceOAuthError::InvalidCredential);
        }
        storage
            .write_resource(&key, &updated)
            .map_err(|_error| ResourceOAuthError::Storage)?;
        Ok(ResourceToken(updated))
    }

    fn validate(&self, auth: &Authentication) -> Result<i64, ResourceOAuthError> {
        let Authentication::OAuth {
            client_id,
            token_endpoint,
            access_token,
            expires_at,
            ..
        } = auth
        else {
            return Err(ResourceOAuthError::InvalidCredential);
        };
        let endpoint =
            Url::parse(token_endpoint).map_err(|_error| ResourceOAuthError::InvalidCredential)?;
        let issuer =
            Url::parse(&self.issuer).map_err(|_error| ResourceOAuthError::InvalidConfiguration)?;
        if client_id != &self.client_id
            || access_token.is_empty()
            || access_token.len() > 65536
            || !access_token.bytes().all(|b| {
                b.is_ascii_alphanumeric()
                    || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/' | b'=')
            })
            || !secure_url(&endpoint)
            || endpoint.fragment().is_some()
            || (endpoint.scheme() == "http" && endpoint.origin() != issuer.origin())
        {
            return Err(ResourceOAuthError::InvalidCredential);
        }
        expires_at.ok_or(ResourceOAuthError::InvalidCredential)
    }

    async fn refresh(
        &self,
        auth: &Authentication,
    ) -> Result<Option<Authentication>, ResourceOAuthError> {
        let Authentication::OAuth {
            refresh_token: Some(refresh_token),
            token_endpoint,
            revocation_endpoint,
            ..
        } = auth
        else {
            return Ok(None);
        };
        if refresh_token.is_empty() {
            return Ok(None);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_error| ResourceOAuthError::RefreshFailed)?;
        // Hydra restores the original audience from the refresh grant. Also send
        // the exact resource context; never reuse a channel's refresh token.
        let mut response = client
            .post(token_endpoint)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", &self.client_id),
                ("audience", &self.audience),
            ])
            .send()
            .await
            .map_err(|_error| ResourceOAuthError::RefreshFailed)?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_error| ResourceOAuthError::RefreshFailed)?
        {
            if bytes.len().saturating_add(chunk.len()) > 65536 {
                return Err(ResourceOAuthError::RefreshFailed);
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            if status == reqwest::StatusCode::BAD_REQUEST
                && serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(str::to_owned))
                    .as_deref()
                    == Some("invalid_grant")
            {
                return Ok(None);
            }
            return Err(ResourceOAuthError::RefreshFailed);
        }
        let response: RefreshResponse =
            serde_json::from_slice(&bytes).map_err(|_error| ResourceOAuthError::RefreshFailed)?;
        if !response.token_type.eq_ignore_ascii_case("bearer")
            || response.refresh_token.as_deref() == Some("")
        {
            return Err(ResourceOAuthError::InvalidCredential);
        }
        let expires_at = response
            .expires_in
            .map(|duration| {
                now()
                    .checked_add(duration)
                    .filter(|_| duration > 0)
                    .ok_or(ResourceOAuthError::InvalidCredential)
            })
            .transpose()?;
        Ok(Some(Authentication::OAuth {
            access_token: response.access_token,
            refresh_token: response
                .refresh_token
                .or_else(|| Some(refresh_token.clone())),
            expires_at,
            token_endpoint: token_endpoint.clone(),
            revocation_endpoint: revocation_endpoint.clone(),
            client_id: self.client_id.clone(),
        }))
    }
}

fn secure_url(url: &Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some()
        && (url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
                || url.scheme() == "http"
                    && matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())))
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
#[derive(Deserialize)]
struct RefreshResponse {
    access_token: String,
    token_type: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

#[cfg(test)]
mod tests;
