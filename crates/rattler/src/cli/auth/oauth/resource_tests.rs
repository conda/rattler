use super::*;

#[test]
fn resource_flow_requires_bearer_token_type() {
    let other = serde_json::from_str("\"MAC\"").unwrap();
    assert!(require_resource_bearer(Some("audit"), &other).is_err());
    assert!(require_resource_bearer(Some("audit"), &CoreTokenType::Bearer).is_ok());
    assert!(require_resource_bearer(None, &other).is_ok());
}

#[test]
fn auth_code_audience_is_exact_and_encoded_without_changing_state() {
    let mut url =
        Url::parse("https://issuer.example/authorize?state=fixture&code_challenge=challenge")
            .unwrap();
    append_audience(&mut url, Some("https://AUDIT.example/path/?x=y&other=z")).unwrap();
    let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(pairs["audience"], "https://AUDIT.example/path/?x=y&other=z");
    assert_eq!(pairs["state"], "fixture");
    assert_eq!(pairs["code_challenge"], "challenge");
    assert!(append_audience(&mut url, Some("other")).is_err());
}

#[tokio::test]
async fn device_flow_requests_audience_and_persists_only_resource_key() {
    use axum::{
        Json, Router,
        extract::Form,
        routing::{get, post},
    };
    use rattler_networking::authentication_storage::backends::memory::MemoryStorage;
    use serde_json::json;
    use std::{collections::HashMap, sync::Arc};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let discovery = json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"jwks_uri":format!("{issuer}/jwks"),"device_authorization_endpoint":format!("{issuer}/device"),"response_types_supported":["code"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["RS256"]});
    let app=Router::new()
        .route("/.well-known/openid-configuration", get(move || {let discovery=discovery.clone(); async move {Json(discovery)}}))
        .route("/jwks", get(|| async {Json(json!({"keys":[]}))}))
        .route("/device", post(|Form(form):Form<HashMap<String,String>>| async move {
            assert_eq!(form["audience"], "https://audit.example");
            assert_eq!(form["client_id"], "rattler");
            assert!(form["scope"].contains("offline_access"));
            Json(json!({"device_code":"fixture-device", "user_code":"TEST", "verification_uri":"https://issuer.example/verify", "expires_in":60,"interval":0}))
        }))
        .route("/token", post(|Form(form):Form<HashMap<String,String>>| async move {
            assert_eq!(form["grant_type"], "urn:ietf:params:oauth:grant-type:device_code");
            Json(json!({"access_token":"fixture-opaque", "refresh_token":"fixture-refresh", "token_type":"Bearer", "expires_in":3600}))
        }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut storage = AuthenticationStorage::empty();
    storage.add_backend(Arc::new(MemoryStorage::new()));
    let config = || OAuthConfig {
        issuer_url: issuer.clone(),
        client_id: "rattler".into(),
        client_secret: None,
        flow: OAuthFlow::DeviceCode,
        scopes: HashSet::new(),
        redirect_uri: None,
        user_agent: None,
        callback_page: None,
    };
    let token = ensure_oauth_audience(
        config(),
        "https://audit.example",
        &storage,
        ResourceInteraction::Allow,
        false,
    )
    .await
    .unwrap();
    assert_eq!(token.access_token(), "fixture-opaque");
    assert!(storage.get("127.0.0.1").unwrap().is_none());
    let entries = storage.backends[0].list().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].0.starts_with("oauth-resource-v1:"));
    server.abort();
    let _ = server.await;

    // The existing config is the only source of issuer/client identity. No
    // separate resource object or live provider is needed to reuse the grant.
    let cached = ensure_oauth_audience(
        config(),
        "https://audit.example",
        &storage,
        ResourceInteraction::Deny,
        true,
    )
    .await
    .unwrap();
    assert_eq!(cached.access_token(), "fixture-opaque");
    let mut other_client = config();
    other_client.client_id = "another-client".into();
    let mut other_issuer = config();
    other_issuer.issuer_url.push_str("/another-issuer");
    for (config, audience) in [
        (config(), "https://another-audit.example"),
        (other_client, "https://audit.example"),
        (other_issuer, "https://audit.example"),
    ] {
        assert_eq!(
            ensure_oauth_audience(config, audience, &storage, ResourceInteraction::Allow, true,)
                .await
                .unwrap_err(),
            ResourceOAuthError::AuthorizationRequired
        );
    }
    let mut confidential = config();
    confidential.client_secret = Some("fixture-secret".into());
    assert_eq!(
        ensure_oauth_audience(
            confidential,
            "https://audit.example",
            &storage,
            ResourceInteraction::Deny,
            true,
        )
        .await
        .unwrap_err(),
        ResourceOAuthError::InvalidConfiguration
    );
}
