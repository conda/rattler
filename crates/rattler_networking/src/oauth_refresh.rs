//! Refresh logic for `Authentication::OAuth` credentials.

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

/// Refresh an OAuth token if it is expired (or close to expiring) and
/// store the refreshed credential back to `storage`.
pub async fn maybe_refresh_oauth(
    storage: &AuthenticationStorage,
    auth: Authentication,
    matched_key: &str,
) -> Option<Authentication> {
    let Authentication::OAuth {
        access_token: _,
        ref refresh_token,
        expires_at,
        ref token_endpoint,
        ref revocation_endpoint,
        ref client_id,
    } = auth
    else {
        return Some(auth);
    };

    let is_expired = expires_at.is_some_and(|exp| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        exp - now < EXPIRY_SKEW_SECONDS
    });

    if !is_expired {
        return Some(auth);
    }

    let Some(refresh_token_val) = refresh_token.as_deref() else {
        tracing::warn!("OAuth token is expired but no refresh token is available");
        return Some(auth);
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
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!("Failed to refresh OAuth token: {e}");
            return Some(auth);
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
        tracing::warn!("OAuth token refresh failed ({hint})");
        return Some(auth);
    }

    let token_response: TokenRefreshResponse = match response.json().await {
        Ok(body) => body,
        Err(e) => {
            tracing::warn!("Failed to read OAuth refresh response body: {e}");
            return Some(auth);
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

    Some(refreshed)
}
