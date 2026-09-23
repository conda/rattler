use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;

use super::*;

fn config() -> OAuthConfig {
    crate::cli::auth::oauth_config_for_host("prefix.dev", &["basilisk:query"]).unwrap()
}

fn auth(claims: Value) -> Authentication {
    Authentication::OAuth {
        access_token: format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        ),
        refresh_token: Some("test-refresh".into()),
        expires_at: None,
        token_endpoint: "https://prefix.dev/oauth2/token".into(),
        revocation_endpoint: None,
        client_id: "rattler".into(),
    }
}

fn claims(scopes: Value) -> Value {
    json!({"iss": "https://prefix.dev", "exp": 4_000_000_000_i64, "scope": scopes})
}

fn grant_for(config: &OAuthConfig) -> Authentication {
    auth(claims(json!(config.scopes)))
}

#[test]
fn audit_configuration_is_browser_first_and_does_not_request_channel_writes() {
    let config = config();
    assert_eq!(config.flow, super::super::OAuthFlow::Auto);
    assert!(config.scopes.contains("basilisk:query"));
    assert!(config.scopes.contains("offline_access"));
    assert!(!config.scopes.contains("channel:upload"));
    assert!(
        crate::cli::auth::oauth_config_for_host("untrusted.example", &["basilisk:query"]).is_none()
    );
}

#[tokio::test]
async fn fresh_complete_grant_is_reused_even_without_interaction() {
    let current = grant_for(&config());
    let result = ensure_with_login(
        config(),
        Some(&current),
        OAuthInteraction::Deny,
        |_| async { panic!("must not authorize when scopes are already granted") },
    )
    .await
    .unwrap();
    assert_eq!(result, current);
}

#[tokio::test]
async fn missing_scope_requests_existing_permissions_plus_command_scopes_once() {
    for scopes in [
        json!("openid channel:read channel:upload custom:permission"),
        json!([
            "openid",
            "channel:read",
            "channel:upload",
            "custom:permission"
        ]),
    ] {
        let current = auth(claims(scopes));
        let original = current.clone();
        let result = ensure_with_login(
            config(),
            Some(&current),
            OAuthInteraction::Allow,
            |config| async move {
                for scope in [
                    "channel:read",
                    "channel:upload",
                    "custom:permission",
                    "basilisk:query",
                    "offline_access",
                ] {
                    assert!(config.scopes.contains(scope), "missing {scope}");
                }
                Ok(grant_for(&config))
            },
        )
        .await
        .unwrap();
        assert!(grant(&result).unwrap().scopes.contains("basilisk:query"));
        assert_eq!(current, original);
    }
}

#[tokio::test]
async fn scp_alias_supports_strings_and_arrays_without_losing_permissions() {
    for scopes in [
        json!("channel:read channel:upload"),
        json!(["channel:read", "channel:upload"]),
    ] {
        let mut value = claims(json!("openid"));
        value["scp"] = scopes;
        let current = auth(value);
        ensure_with_login(
            config(),
            Some(&current),
            OAuthInteraction::Allow,
            |config| async move {
                assert!(config.scopes.contains("channel:upload"));
                assert!(config.scopes.contains("channel:read"));
                Ok(grant_for(&config))
            },
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn first_login_requests_only_command_and_identity_scopes() {
    ensure_with_login(
        config(),
        None,
        OAuthInteraction::Allow,
        |config| async move {
            assert!(!config.scopes.contains("channel:upload"));
            assert!(config.scopes.contains("basilisk:query"));
            Ok(grant_for(&config))
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn noninteractive_missing_or_incomplete_grants_never_start_login() {
    let old = auth(claims(json!("openid channel:read")));
    for current in [None, Some(&old)] {
        let result = ensure_with_login(config(), current, OAuthInteraction::Deny, |_| async {
            panic!("CI must not start a browser or device flow")
        })
        .await;
        assert!(matches!(
            result,
            Err(EnsureScopesError::AuthorizationRequired)
        ));
    }
}

#[tokio::test]
async fn cancelled_or_failed_authorization_leaves_old_credentials_unchanged() {
    let current = auth(claims(json!("openid channel:upload")));
    let original = current.clone();
    let result = ensure_with_login(
        config(),
        Some(&current),
        OAuthInteraction::Allow,
        |_| async { Err(OAuthError::Authorization("access_denied".into())) },
    )
    .await;
    assert!(matches!(result, Err(EnsureScopesError::OAuth(_))));
    assert_eq!(current, original);
}

#[tokio::test]
async fn partial_consent_cannot_drop_existing_permissions_or_required_scope() {
    let current = auth(claims(json!("openid channel:upload")));
    for missing in ["channel:upload", "basilisk:query"] {
        let result = ensure_with_login(
            config(),
            Some(&current),
            OAuthInteraction::Allow,
            |mut config| async move {
                config.scopes.remove(missing);
                Ok(grant_for(&config))
            },
        )
        .await;
        assert!(matches!(result, Err(EnsureScopesError::IncompleteGrant)));
    }
}

#[tokio::test]
async fn malformed_or_opaque_existing_grants_are_not_overwritten() {
    for current in [
        Authentication::BearerToken("opaque-token".into()),
        auth(json!({"iss": "https://prefix.dev", "exp": 4_000_000_000_i64})),
        auth(claims(json!(["channel:read", 42]))),
        auth(claims(Value::Null)),
    ] {
        let result = ensure_with_login(
            config(),
            Some(&current),
            OAuthInteraction::Allow,
            |_| async { panic!("cannot safely reconstruct existing permissions") },
        )
        .await;
        assert!(matches!(result, Err(EnsureScopesError::UnknownGrant)));
    }
}

#[tokio::test]
async fn other_issuer_or_client_is_not_silently_replaced() {
    let mut other_issuer = claims(json!("channel:read"));
    other_issuer["iss"] = json!("https://other.example");
    let mut other_client = auth(claims(json!("channel:read")));
    if let Authentication::OAuth { client_id, .. } = &mut other_client {
        *client_id = "another-client".into();
    }
    for current in [auth(other_issuer), other_client] {
        let result = ensure_with_login(
            config(),
            Some(&current),
            OAuthInteraction::Allow,
            |_| async { panic!("must not switch issuer or client") },
        )
        .await;
        assert!(matches!(result, Err(EnsureScopesError::DifferentClient)));
    }
}

#[tokio::test]
async fn expired_or_invalid_replacement_grants_are_rejected() {
    for invalid in ["expired", "issuer", "client", "opaque"] {
        let result = ensure_with_login(
            config(),
            None,
            OAuthInteraction::Allow,
            |config| async move {
                let mut value = claims(json!(config.scopes));
                if invalid == "expired" {
                    value["exp"] = json!(1);
                }
                if invalid == "issuer" {
                    value["iss"] = json!("https://other.example");
                }
                let mut result = auth(value);
                if invalid == "client"
                    && let Authentication::OAuth { client_id, .. } = &mut result
                {
                    *client_id = "other".into();
                }
                if invalid == "opaque" {
                    result = Authentication::BearerToken("opaque".into());
                }
                Ok(result)
            },
        )
        .await;
        assert!(matches!(result, Err(EnsureScopesError::IncompleteGrant)));
    }
}

#[tokio::test]
async fn expired_complete_grant_requires_reauthorization_if_refresh_was_not_possible() {
    let mut current = grant_for(&config());
    if let Authentication::OAuth { expires_at, .. } = &mut current {
        *expires_at = Some(1);
    }
    let result = ensure_with_login(
        config(),
        Some(&current),
        OAuthInteraction::Deny,
        |_| async { panic!("expired CI credentials must not start login") },
    )
    .await;
    assert!(matches!(
        result,
        Err(EnsureScopesError::AuthorizationRequired)
    ));
}
