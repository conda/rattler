use super::*;
use serde_json::json;

fn legacy(token: &str) -> Value {
    json!({"OAuth": {
        "access_token": token, "refresh_token": "refresh", "expires_at": null,
        "token_endpoint": "https://issuer.example/token", "revocation_endpoint": null,
        "client_id": "test-client"
    }})
}

#[test]
fn legacy_credentials_round_trip_without_metadata() {
    let value = legacy("opaque");
    let auth: Authentication = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(auth.oauth_scopes(), None);
    assert_eq!(auth.oauth_issuer_url(), None);
    assert_eq!(serde_json::to_value(auth).unwrap(), value);
}

#[test]
fn opaque_grant_metadata_round_trips_including_empty_grants() {
    for scopes in [json!(["custom:read", "custom:write"]), json!([])] {
        let mut value = legacy("opaque");
        value["OAuth"]["issuer_url"] = json!("https://issuer.example");
        value["OAuth"]["scopes"] = scopes.clone();
        let auth: Authentication = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            auth.oauth_issuer_url().as_deref(),
            Some("https://issuer.example")
        );
        assert_eq!(
            serde_json::to_value(auth.oauth_scopes().unwrap()).unwrap(),
            scopes
        );
        assert_eq!(serde_json::to_value(auth).unwrap(), value);
    }
}

#[test]
fn legacy_jwt_claims_are_a_fallback_not_an_override() {
    let claims = json!({"iss":"https://old.example", "scope":"old:read", "scp":["old:write"]});
    let token = format!(
        "e30.{}.signature",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    );
    let mut value = legacy(&token);
    let auth: Authentication = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(auth.oauth_scopes().unwrap(), ["old:read", "old:write"]);
    assert_eq!(
        auth.oauth_issuer_url().as_deref(),
        Some("https://old.example")
    );
    value["OAuth"]["issuer_url"] = json!("https://issuer.example");
    value["OAuth"]["scopes"] = json!([]);
    let auth: Authentication = serde_json::from_value(value).unwrap();
    assert_eq!(auth.oauth_scopes(), Some(vec![]));
    assert_eq!(
        auth.oauth_issuer_url().as_deref(),
        Some("https://issuer.example")
    );
}

#[test]
fn malformed_or_missing_legacy_scopes_are_unknown() {
    for claims in [json!({}), json!({"scope":42}), json!({"scp":["read", 42]})] {
        let token = format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        );
        let auth: Authentication = serde_json::from_value(legacy(&token)).unwrap();
        assert_eq!(auth.oauth_scopes(), None);
    }
}
