//! Refresh logic for `Authentication::OAuth` credentials.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
};

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

/// Single-flight gates keyed by the storage key, so concurrent requests for
/// the same credential share one refresh attempt instead of each hitting the
/// token endpoint (and logging) independently.
static REFRESH_GATES: LazyLock<Mutex<HashMap<String, Arc<futures::lock::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Refresh attempts that already failed this process, keyed by
/// `<storage key>\0<refresh token>` (or a `<no-refresh>` sentinel when there
/// is no refresh token). We neither retry nor re-log these, so a dead
/// credential produces a single warning per process rather than one per
/// request.
static FAILED_REFRESHES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Whether an `OAuth` credential is expired (or within the refresh skew).
/// Non-OAuth credentials are never considered expired here.
fn oauth_is_expired(auth: &Authentication) -> bool {
    match auth {
        Authentication::OAuth { expires_at, .. } => {
            expires_at.is_some_and(|exp| exp - now_secs() < EXPIRY_SKEW_SECONDS)
        }
        _ => false,
    }
}

/// Record `key` as a failed refresh. Returns `true` only the first time the
/// key is seen this process, so callers log at most once.
fn record_failure_once(key: &str) -> bool {
    FAILED_REFRESHES
        .lock()
        .expect("OAuth refresh failure cache poisoned")
        .insert(key.to_string())
}

fn is_failed(key: &str) -> bool {
    FAILED_REFRESHES
        .lock()
        .expect("OAuth refresh failure cache poisoned")
        .contains(key)
}

/// Refresh an OAuth token if it is expired (or close to expiring) and
/// store the refreshed credential back to `storage`.
///
/// Returns:
/// - `Some(credential)` for a non-OAuth credential, an OAuth credential that
///   is still valid, or a successfully refreshed one;
/// - `None` when an OAuth credential is expired and could **not** be refreshed
///   (no refresh token, network error, or the server rejected it). In that
///   case the request is sent unauthenticated so downstream middleware (e.g.
///   the auth-challenge middleware) can take over; if it still fails, the
///   caller can tell the user to re-authenticate.
///
/// Concurrent calls for the same credential are coalesced: only one performs
/// the refresh, the rest wait and reuse its result. A refresh that fails is
/// recorded so it is neither retried nor re-logged for the rest of the
/// process.
pub async fn maybe_refresh_oauth(
    storage: &AuthenticationStorage,
    auth: Authentication,
    matched_key: &str,
) -> Option<Authentication> {
    let Authentication::OAuth {
        access_token: _,
        ref refresh_token,
        expires_at: _,
        ref token_endpoint,
        ref revocation_endpoint,
        ref client_id,
    } = auth
    else {
        return Some(auth);
    };

    if !oauth_is_expired(&auth) {
        return Some(auth);
    }

    let Some(refresh_token_val) = refresh_token.as_deref() else {
        if record_failure_once(&format!("{matched_key}\0<no-refresh>")) {
            tracing::warn!("OAuth token is expired but no refresh token is available");
        }
        return None;
    };

    let failed_key = format!("{matched_key}\0{refresh_token_val}");

    // Fast path: this exact refresh token already failed this process.
    if is_failed(&failed_key) {
        return None;
    }

    // Single-flight: concurrent requests for the same credential coalesce on
    // one gate so only the first performs the refresh.
    let gate = {
        let mut gates = REFRESH_GATES
            .lock()
            .expect("OAuth refresh gate map poisoned");
        gates
            .entry(matched_key.to_string())
            .or_insert_with(|| Arc::new(futures::lock::Mutex::new(())))
            .clone()
    };
    let _guard = gate.lock().await;

    // Re-check now that we hold the gate: a sibling may have refreshed the
    // credential (use its fresh result) or already failed (drop it).
    if let Ok(Some(current)) = storage.get(matched_key)
        && !oauth_is_expired(&current)
    {
        return Some(current);
    }
    if is_failed(&failed_key) {
        return None;
    }

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
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            if record_failure_once(&failed_key) {
                tracing::warn!("Failed to refresh OAuth token: {e}");
            }
            return None;
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let hint = match response.json::<serde_json::Value>().await {
            Ok(body) => {
                let error_code = body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                if error_code == "invalid_grant" {
                    "refresh token is expired or revoked — please re-authenticate".to_string()
                } else {
                    format!("error code: {error_code}")
                }
            }
            Err(_) => format!("HTTP {status}"),
        };
        if record_failure_once(&failed_key) {
            tracing::warn!("OAuth token refresh failed ({hint})");
        }
        return None;
    }

    let token_response: TokenRefreshResponse = match response.json().await {
        Ok(body) => body,
        Err(e) => {
            if record_failure_once(&failed_key) {
                tracing::warn!("Failed to read OAuth refresh response body: {e}");
            }
            return None;
        }
    };

    let new_expires_at = token_response.expires_in.map(|secs| now_secs() + secs);

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

    Some(refreshed)
}
