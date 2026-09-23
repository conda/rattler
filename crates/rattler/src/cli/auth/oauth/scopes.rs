//! Incremental authorization for commands needing additional OAuth permissions.
//!
//! Stored grant metadata (or unverified JWT claims for legacy credentials) is
//! a UX hint, not an authorization boundary. The resource server must still
//! validate every access token. Opaque access tokens use stored metadata.
//! This module neither
//! reads nor writes credential storage: callers refresh credentials first, then
//! persist the returned grant only after this operation succeeds.
//!
//! A command integration can use the following pattern. The resource URL must
//! be trusted separately; never send this token to an arbitrary audit mirror.
//!
//! ```no_run
//! use std::io::{self, IsTerminal};
//! use rattler::cli::auth::{oauth_config_for_host, oauth::{ensure_oauth_scopes, OAuthInteraction}};
//! use rattler_networking::{Authentication, AuthenticationStorage};
//!
//! async fn authorize_audit(
//!     storage: &AuthenticationStorage,
//!     offline: bool,
//! ) -> Result<Authentication, Box<dyn std::error::Error>> {
//!     let issuer = "https://prefix.dev";
//!     let url = url::Url::parse(issuer)?;
//!     let entry = storage.get_by_url_with_host_strict(&url)?;
//!     let key = entry.as_ref().map(|(key, _)| key.as_str()).unwrap_or("prefix.dev");
//!     let (_, current) = if offline {
//!         storage.get_by_url(issuer)?
//!     } else {
//!         storage.get_by_url_refreshed(issuer).await?
//!     };
//!     // Refresh failure must not discard the old grant's permission list.
//!     let current = current.or_else(|| entry.as_ref().map(|(_, auth)| auth.clone()));
//!     let interaction = if !offline && std::env::var_os("CI").is_none()
//!         && io::stdin().is_terminal() && io::stderr().is_terminal()
//!     {
//!         OAuthInteraction::Allow
//!     } else {
//!         OAuthInteraction::Deny
//!     };
//!     let config = oauth_config_for_host("prefix.dev", &["basilisk:query"]).unwrap();
//!     let authorized = ensure_oauth_scopes(config, current.as_ref(), interaction).await?;
//!     if current.as_ref() != Some(&authorized) {
//!         storage.store(key, &authorized)?;
//!     }
//!     Ok(authorized)
//! }
//! ```

use std::{
    collections::HashSet,
    future::Future,
    time::{SystemTime, UNIX_EPOCH},
};

use rattler_networking::{Authentication, authentication_storage::authentication::is_oidc_scope};

use super::{OAuthConfig, OAuthError, perform_oauth_login};

/// Whether a command may start browser/device authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthInteraction {
    /// The caller has established that an interactive login is appropriate.
    Allow,
    /// CI, noninteractive, or offline execution must not start authorization.
    Deny,
}

/// Incremental-authorization errors deliberately contain no credentials.
#[derive(Debug, thiserror::Error)]
pub enum EnsureScopesError {
    /// Interactive authorization is required, but forbidden by the caller.
    #[error(
        "Additional OAuth authorization is required; rerun the command in an interactive terminal, or provision a token granting the required scopes for CI"
    )]
    AuthorizationRequired,
    /// An unknown existing grant cannot safely be replaced without losing access.
    #[error(
        "Cannot determine the existing OAuth grant; credentials are unchanged. Use credentials with known granted-scope metadata, or explicitly use auth login --oauth --replace with all desired permissions"
    )]
    UnknownGrant,
    /// The caller must not replace another issuer's or client's credentials.
    #[error(
        "Stored OAuth credentials belong to another issuer or client. To intentionally replace them, use explicit auth login --oauth --replace; automatic scope upgrades cannot switch clients"
    )]
    DifferentClient,
    /// Reject partial consent rather than silently dropping prior permissions.
    #[error(
        "OAuth authorization did not return a usable token with all requested permissions; leaving credentials unchanged"
    )]
    IncompleteGrant,
    /// Authorization failed or the user declined it.
    #[error(transparent)]
    OAuth(#[from] OAuthError),
}

/// Ensure a command's requested scopes, prompting at most once if permitted.
///
/// `config.scopes` is the command's desired scope set. Authorization requests
/// include the union of these scopes and every scope in the existing grant.
/// Standard OIDC scopes are requested and remembered but are not required in
/// access-token scope metadata. Only resource permissions are checked for
/// completeness and cached-token reuse. This does not guarantee availability
/// of a refresh token or verified identity; those are separate capabilities.
/// A fresh matching token is reused without opening a browser. Use
/// [`super::OAuthFlow::Auto`] for browser login with device-code fallback.
///
/// Pass credentials obtained via `AuthenticationStorage::get_by_url_refreshed`
/// so normal expiry can be handled silently first. On success, persist the
/// result under the original storage key before making the API call. On any
/// error (including cancellation or partial consent), do not overwrite or
/// delete the old credentials. The function does not retry a rejected API call.
///
/// Callers must choose `Deny` in CI/noninteractive/offline contexts and must
/// only construct configurations from trusted issuer/client settings, never
/// blindly from an arbitrary resource server's authentication challenge.
pub async fn ensure_oauth_scopes(
    config: OAuthConfig,
    current: Option<&Authentication>,
    interaction: OAuthInteraction,
) -> Result<Authentication, EnsureScopesError> {
    ensure_with_login(config, current, interaction, perform_oauth_login).await
}

async fn ensure_with_login<F, Fut>(
    config: OAuthConfig,
    current: Option<&Authentication>,
    interaction: OAuthInteraction,
    login: F,
) -> Result<Authentication, EnsureScopesError>
where
    F: FnOnce(OAuthConfig) -> Fut,
    Fut: Future<Output = Result<Authentication, OAuthError>>,
{
    authorize_with_login(config, current, interaction, true, login).await
}

/// Explicit login always reauthorizes, even if the existing grant is sufficient.
pub(in crate::cli::auth) async fn reauthorize_with_login<F, Fut>(
    config: OAuthConfig,
    current: Option<&Authentication>,
    login: F,
) -> Result<Authentication, EnsureScopesError>
where
    F: FnOnce(OAuthConfig) -> Fut,
    Fut: Future<Output = Result<Authentication, OAuthError>>,
{
    authorize_with_login(config, current, OAuthInteraction::Allow, false, login).await
}

async fn authorize_with_login<F, Fut>(
    mut config: OAuthConfig,
    current: Option<&Authentication>,
    interaction: OAuthInteraction,
    reuse_current: bool,
    login: F,
) -> Result<Authentication, EnsureScopesError>
where
    F: FnOnce(OAuthConfig) -> Fut,
    Fut: Future<Output = Result<Authentication, OAuthError>>,
{
    if config.scopes.is_empty() {
        config.scopes.extend(
            super::DEFAULT_OAUTH_SCOPES
                .iter()
                .map(|scope| (*scope).to_owned()),
        );
    }
    if let Some(current) = current {
        let grant = grant(current).ok_or(EnsureScopesError::UnknownGrant)?;
        if !same_issuer(&grant.issuer, &config.issuer_url) || grant.client_id != config.client_id {
            return Err(EnsureScopesError::DifferentClient);
        }
        if reuse_current
            && grant.is_fresh()
            && resource_scopes(&config.scopes).is_subset(&grant.scopes)
        {
            return Ok(current.clone());
        }
        config.scopes.extend(grant.scopes);
        config.scopes.extend(grant.oidc_scopes);
    }
    if interaction == OAuthInteraction::Deny {
        return Err(EnsureScopesError::AuthorizationRequired);
    }

    let requested = resource_scopes(&config.scopes);
    let issuer = config.issuer_url.clone();
    let client_id = config.client_id.clone();
    let updated = login(config).await?;
    let grant = grant(&updated).ok_or(EnsureScopesError::IncompleteGrant)?;
    if !same_issuer(&grant.issuer, &issuer)
        || grant.client_id != client_id
        || !grant.is_fresh()
        || !requested.is_subset(&grant.scopes)
    {
        return Err(EnsureScopesError::IncompleteGrant);
    }
    Ok(updated)
}

// Match URL normalization used by OIDC discovery: a root slash is equivalent,
// but issuer paths (including their trailing slash) remain distinct.
fn same_issuer(left: &str, right: &str) -> bool {
    url::Url::parse(left)
        .ok()
        .zip(url::Url::parse(right).ok())
        .is_some_and(|(left, right)| left == right)
}

fn resource_scopes(scopes: &HashSet<String>) -> HashSet<String> {
    scopes
        .iter()
        .filter(|scope| !is_oidc_scope(scope))
        .cloned()
        .collect()
}

struct Grant {
    issuer: String,
    client_id: String,
    scopes: HashSet<String>,
    oidc_scopes: HashSet<String>,
    expires_at: Option<i64>,
}

impl Grant {
    fn is_fresh(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // As in normal OAuth refresh, no expiry means no known expiration.
        self.expires_at
            .is_none_or(|expiry| expiry > 0 && expiry as u64 > now.saturating_add(30))
    }
}

fn grant(auth: &Authentication) -> Option<Grant> {
    let Authentication::OAuth {
        access_token,
        client_id,
        expires_at,
        oidc,
        ..
    } = auth
    else {
        return None;
    };
    let token_expiry = super::super::jwt_claims(access_token)
        .and_then(|claims| claims.get("exp").and_then(serde_json::Value::as_i64));
    let scopes: HashSet<_> = auth.oauth_scopes()?.into_iter().collect();
    let mut oidc_scopes: HashSet<_> = scopes
        .iter()
        .filter(|scope| is_oidc_scope(scope))
        .cloned()
        .collect();
    if let Some(oidc) = oidc {
        oidc_scopes.extend(
            oidc.requested_scopes
                .iter()
                .filter(|scope| is_oidc_scope(scope))
                .cloned(),
        );
    }
    Some(Grant {
        issuer: auth.oauth_issuer_url()?,
        client_id: client_id.clone(),
        scopes: resource_scopes(&scopes),
        oidc_scopes,
        expires_at: match (*expires_at, token_expiry) {
            (Some(stored), Some(token)) => Some(stored.min(token)),
            (stored, token) => stored.or(token),
        },
    })
}

#[cfg(test)]
mod tests;
