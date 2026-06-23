//! Refresh logic for `Authentication::OAuth` credentials.

use std::fmt;

use serde::Deserialize;

use crate::{Authentication, AuthenticationStorage};

/// Standard OAuth 2.0 token response.
#[derive(Deserialize)]
struct TokenRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

/// Number of seconds before `expires_at` at which a token is considered
/// "about to expire"
const EXPIRY_SKEW_SECONDS: i64 = 300;

/// Maximum time to wait for the token endpoint to respond to a refresh request.
/// Without this, a hung authorization server would stall the request that
/// triggered the refresh indefinitely.
const REFRESH_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn now_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn oauth_expires_at(auth: &Authentication) -> Option<i64> {
    match auth {
        Authentication::OAuth { expires_at, .. } => *expires_at,
        _ => None,
    }
}

fn needs_refresh(auth: &Authentication) -> bool {
    oauth_expires_at(auth).is_some_and(|exp| exp - now_unix_timestamp() < EXPIRY_SKEW_SECONDS)
}

fn is_expired(auth: &Authentication) -> bool {
    oauth_expires_at(auth).is_some_and(|exp| exp <= now_unix_timestamp())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OAuthRefreshFailure {
    MissingRefreshToken,
    ReauthenticationRequired { reason: String },
    Transient { reason: String },
    InvalidResponse { reason: String },
}

impl fmt::Display for OAuthRefreshFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OAuthRefreshFailure::MissingRefreshToken => {
                write!(
                    f,
                    "OAuth token is expired but no refresh token is available"
                )
            }
            OAuthRefreshFailure::ReauthenticationRequired { reason }
            | OAuthRefreshFailure::Transient { reason }
            | OAuthRefreshFailure::InvalidResponse { reason } => write!(f, "{reason}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OAuthRefreshOutcome {
    authentication: Option<Authentication>,
    failure: Option<OAuthRefreshFailure>,
}

impl OAuthRefreshOutcome {
    pub(crate) fn into_authentication(self) -> Option<Authentication> {
        self.authentication
    }

    pub(crate) fn failure(&self) -> Option<&OAuthRefreshFailure> {
        self.failure.as_ref()
    }
}

fn auth_outcome(auth: Authentication) -> OAuthRefreshOutcome {
    OAuthRefreshOutcome {
        authentication: Some(auth),
        failure: None,
    }
}

fn stale_auth_fallback(auth: Authentication, failure: OAuthRefreshFailure) -> OAuthRefreshOutcome {
    if is_expired(&auth) {
        tracing::warn!(
            "OAuth access token is expired and could not be refreshed; not attaching it"
        );
        OAuthRefreshOutcome {
            authentication: None,
            failure: Some(failure),
        }
    } else {
        OAuthRefreshOutcome {
            authentication: Some(auth),
            failure: Some(failure),
        }
    }
}

/// Refresh an OAuth token if it is expired (or close to expiring) and
/// store the refreshed credential back to `storage`.
pub(crate) async fn maybe_refresh_oauth(
    storage: &AuthenticationStorage,
    auth: Authentication,
    matched_key: &str,
) -> OAuthRefreshOutcome {
    if !matches!(auth, Authentication::OAuth { .. }) {
        return auth_outcome(auth);
    }

    if !needs_refresh(&auth) {
        return auth_outcome(auth);
    }

    let refresh_lock = storage.oauth_refresh_lock(matched_key);
    let _refresh_guard = refresh_lock.lock().await;

    // Another request may have refreshed and stored credentials for this key
    // while we were waiting for the per-key refresh lock. Re-read storage so we
    // don't reuse a refresh token that has already been rotated.
    let auth = match storage.get(matched_key) {
        Ok(Some(current_auth)) => {
            if !matches!(current_auth, Authentication::OAuth { .. })
                || !needs_refresh(&current_auth)
            {
                return auth_outcome(current_auth);
            }
            current_auth
        }
        Ok(None) => auth,
        Err(e) => {
            tracing::warn!("Failed to re-read OAuth credentials before refresh: {e}");
            auth
        }
    };

    let Authentication::OAuth {
        access_token: _,
        ref refresh_token,
        expires_at: _,
        ref token_endpoint,
        ref revocation_endpoint,
        ref client_id,
    } = auth
    else {
        return auth_outcome(auth);
    };

    let Some(refresh_token_val) = refresh_token.as_deref() else {
        let failure = OAuthRefreshFailure::MissingRefreshToken;
        tracing::warn!("{failure}");
        return stale_auth_fallback(auth, failure);
    };

    tracing::debug!("OAuth token expired, attempting refresh");

    let client = reqwest::Client::new();
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token_val),
        ("client_id", client_id),
    ];

    let response = match client
        .post(token_endpoint.as_str())
        .form(&params)
        .timeout(REFRESH_REQUEST_TIMEOUT)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            let failure = OAuthRefreshFailure::Transient {
                reason: format!("Failed to refresh OAuth token: {e}"),
            };
            tracing::warn!("{failure}");
            return stale_auth_fallback(auth, failure);
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let failure = match response.json::<serde_json::Value>().await {
            Ok(body) => {
                let error_code = body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                if error_code == "invalid_grant" {
                    OAuthRefreshFailure::ReauthenticationRequired {
                        reason: "refresh token is expired or revoked — please re-authenticate"
                            .to_string(),
                    }
                } else {
                    OAuthRefreshFailure::Transient {
                        reason: format!("OAuth token refresh failed with error code: {error_code}"),
                    }
                }
            }
            Err(_) => OAuthRefreshFailure::Transient {
                reason: format!("OAuth token refresh failed with HTTP {status}"),
            },
        };
        tracing::warn!("OAuth token refresh failed ({failure})");
        return stale_auth_fallback(auth, failure);
    }

    let token_response: TokenRefreshResponse = match response.json().await {
        Ok(body) => body,
        Err(e) => {
            let failure = OAuthRefreshFailure::InvalidResponse {
                reason: format!("Failed to read OAuth refresh response body: {e}"),
            };
            tracing::warn!("{failure}");
            return stale_auth_fallback(auth, failure);
        }
    };

    let new_expires_at = token_response.expires_in.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + secs
    });

    let refreshed = Authentication::OAuth {
        access_token: token_response.access_token,
        refresh_token: token_response
            .refresh_token
            .or_else(|| refresh_token.clone()),
        expires_at: new_expires_at,
        token_endpoint: token_endpoint.clone(),
        revocation_endpoint: revocation_endpoint.clone(),
        client_id: client_id.clone(),
    };

    if let Err(e) = storage.store(matched_key, &refreshed) {
        tracing::warn!("Failed to store refreshed OAuth token: {e}");
    }

    auth_outcome(refreshed)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use axum::{Json, Router, extract::Form, http::StatusCode, routing::post};
    use futures::future::join_all;
    use serde_json::json;

    use super::*;
    use crate::authentication_storage::backends::memory::MemoryStorage;

    fn expired_oauth(token_endpoint: String) -> Authentication {
        Authentication::OAuth {
            access_token: "expired-access-token".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            expires_at: Some(now_unix_timestamp() - 60),
            token_endpoint,
            revocation_endpoint: None,
            client_id: "client-id".to_string(),
        }
    }

    /// An OAuth token inside the refresh skew window (so `needs_refresh` is true)
    /// but not yet expired (so `is_expired` is false).
    fn near_expiry_oauth(token_endpoint: String) -> Authentication {
        Authentication::OAuth {
            access_token: "near-expiry-access-token".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            expires_at: Some(now_unix_timestamp() + 60),
            token_endpoint,
            revocation_endpoint: None,
            client_id: "client-id".to_string(),
        }
    }

    fn auth_storage(host: &str, auth: &Authentication) -> AuthenticationStorage {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        storage.store(host, auth).unwrap();
        storage
    }

    /// Spawn a token endpoint whose handler receives the posted form parameters,
    /// so tests can assert on (and react to) the `refresh_token` that was sent.
    async fn spawn_token_endpoint_with_form(
        handler: impl Fn(HashMap<String, String>) -> (StatusCode, Json<serde_json::Value>)
        + Clone
        + Send
        + Sync
        + 'static,
    ) -> String {
        let router = Router::new().route(
            "/token",
            post({
                let handler = handler.clone();
                move |Form(form): Form<HashMap<String, String>>| {
                    let handler = handler.clone();
                    async move { handler(form) }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}/token")
    }

    async fn spawn_token_endpoint(
        handler: impl Fn() -> (StatusCode, Json<serde_json::Value>) + Clone + Send + Sync + 'static,
    ) -> String {
        let router = Router::new().route(
            "/token",
            post({
                let handler = handler.clone();
                move || {
                    let handler = handler.clone();
                    async move { handler() }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}/token")
    }

    #[tokio::test]
    async fn concurrent_refresh_is_single_flight() {
        let refresh_count = Arc::new(AtomicUsize::new(0));
        let token_endpoint = spawn_token_endpoint({
            let refresh_count = refresh_count.clone();
            move || {
                refresh_count.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(json!({
                        "access_token": "fresh-access-token",
                        "refresh_token": "rotated-refresh-token",
                        "expires_in": 3600,
                    })),
                )
            }
        })
        .await;

        let host = "repo.prefix.dev";
        let expired = expired_oauth(token_endpoint);
        let storage = auth_storage(host, &expired);

        let results =
            join_all((0..8).map(|_| maybe_refresh_oauth(&storage, expired.clone(), host))).await;

        assert_eq!(refresh_count.load(Ordering::SeqCst), 1);
        for result in results {
            assert_eq!(result.failure, None);
            assert!(matches!(
                result.authentication,
                Some(Authentication::OAuth { access_token, .. }) if access_token == "fresh-access-token"
            ));
        }
    }

    #[tokio::test]
    async fn expired_token_is_not_returned_after_refresh_failure() {
        let token_endpoint = spawn_token_endpoint(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_grant" })),
            )
        })
        .await;

        let host = "repo.prefix.dev";
        let expired = expired_oauth(token_endpoint);
        let storage = auth_storage(host, &expired);

        let result = maybe_refresh_oauth(&storage, expired, host).await;

        assert_eq!(result.authentication, None);
        assert!(matches!(
            result.failure,
            Some(OAuthRefreshFailure::ReauthenticationRequired { .. })
        ));
    }

    /// The core property the coalescing gate protects: with refresh-token
    /// rotation enabled, concurrent refreshes must never send a stale refresh
    /// token to the authorization server. The endpoint here rotates its single
    /// valid refresh token on every successful refresh and rejects any other
    /// token with `invalid_grant` — exactly how a rotating server behaves. If
    /// two requests both refreshed (the bug), the second would arrive with the
    /// now-stale original token and be rejected.
    #[tokio::test]
    async fn concurrent_refresh_never_reuses_rotated_token() {
        let valid_refresh_token = Arc::new(Mutex::new("refresh-token".to_string()));
        let success_count = Arc::new(AtomicUsize::new(0));
        let invalid_grant_count = Arc::new(AtomicUsize::new(0));

        let token_endpoint = spawn_token_endpoint_with_form({
            let valid_refresh_token = valid_refresh_token.clone();
            let success_count = success_count.clone();
            let invalid_grant_count = invalid_grant_count.clone();
            move |form| {
                let presented = form.get("refresh_token").cloned().unwrap_or_default();
                let mut valid = valid_refresh_token.lock().unwrap();
                if presented == *valid {
                    // Rotate: the presented token is now spent.
                    let rotated =
                        format!("rotated-{}", success_count.fetch_add(1, Ordering::SeqCst));
                    *valid = rotated.clone();
                    (
                        StatusCode::OK,
                        Json(json!({
                            "access_token": "fresh-access-token",
                            "refresh_token": rotated,
                            "expires_in": 3600,
                        })),
                    )
                } else {
                    invalid_grant_count.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "invalid_grant" })),
                    )
                }
            }
        })
        .await;

        let host = "repo.prefix.dev";
        let expired = expired_oauth(token_endpoint);
        let storage = auth_storage(host, &expired);

        let results =
            join_all((0..8).map(|_| maybe_refresh_oauth(&storage, expired.clone(), host))).await;

        // Exactly one refresh hit the server and no stale token was ever presented.
        assert_eq!(success_count.load(Ordering::SeqCst), 1);
        assert_eq!(invalid_grant_count.load(Ordering::SeqCst), 0);
        for result in results {
            assert_eq!(result.failure, None);
            assert!(matches!(
                result.authentication,
                Some(Authentication::OAuth { access_token, .. }) if access_token == "fresh-access-token"
            ));
        }
    }

    #[tokio::test]
    async fn missing_refresh_token_yields_unauthenticated() {
        let host = "repo.prefix.dev";
        let expired = Authentication::OAuth {
            access_token: "expired-access-token".to_string(),
            refresh_token: None,
            expires_at: Some(now_unix_timestamp() - 60),
            token_endpoint: "http://127.0.0.1:0/token".to_string(),
            revocation_endpoint: None,
            client_id: "client-id".to_string(),
        };
        let storage = auth_storage(host, &expired);

        let result = maybe_refresh_oauth(&storage, expired, host).await;

        assert_eq!(result.authentication, None);
        assert!(matches!(
            result.failure,
            Some(OAuthRefreshFailure::MissingRefreshToken)
        ));
    }

    #[tokio::test]
    async fn transient_failure_on_expired_token_yields_unauthenticated() {
        let token_endpoint = spawn_token_endpoint(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "server_error" })),
            )
        })
        .await;

        let host = "repo.prefix.dev";
        let expired = expired_oauth(token_endpoint);
        let storage = auth_storage(host, &expired);

        let result = maybe_refresh_oauth(&storage, expired, host).await;

        // The token is expired, so a transient failure must not attach it.
        assert_eq!(result.authentication, None);
        assert!(matches!(
            result.failure,
            Some(OAuthRefreshFailure::Transient { .. })
        ));
    }

    #[tokio::test]
    async fn transient_failure_keeps_unexpired_token() {
        let token_endpoint = spawn_token_endpoint(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "server_error" })),
            )
        })
        .await;

        let host = "repo.prefix.dev";
        let near_expiry = near_expiry_oauth(token_endpoint);
        let storage = auth_storage(host, &near_expiry);

        let result = maybe_refresh_oauth(&storage, near_expiry, host).await;

        // The token is within the refresh-skew window but not yet expired, so a
        // transient failure should still attach the existing token and surface
        // the failure as non-fatal.
        assert!(matches!(
            result.authentication,
            Some(Authentication::OAuth { access_token, .. }) if access_token == "near-expiry-access-token"
        ));
        assert!(matches!(
            result.failure,
            Some(OAuthRefreshFailure::Transient { .. })
        ));
    }
}
