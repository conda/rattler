use super::*;
mod regressions;
use crate::authentication_storage::{
    AuthenticationStorageError, StorageBackend,
    backends::memory::{MemoryStorage, MemoryStorageError},
};
use axum::{Json, Router, extract::Form, http::StatusCode, routing::post};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn target() -> AudienceContext {
    AudienceContext::new(
        "https://issuer.example".into(),
        "rattler".into(),
        "https://audit.example".into(),
    )
    .unwrap()
}
fn credential(endpoint: &str, expires: i64) -> Authentication {
    Authentication::OAuth {
        access_token: "fixture-access".into(),
        refresh_token: Some("fixture-refresh".into()),
        expires_at: Some(expires),
        token_endpoint: endpoint.into(),
        revocation_endpoint: None,
        client_id: "rattler".into(),
    }
}
fn storage() -> AuthenticationStorage {
    let mut store = AuthenticationStorage::empty();
    store.add_backend(Arc::new(MemoryStorage::new()));
    store
}
async fn no_login() -> Result<Authentication, ResourceOAuthError> {
    panic!("interactive login must not run")
}
async fn endpoint(
    handler: impl Fn(HashMap<String, String>) -> (StatusCode, Json<serde_json::Value>)
    + Clone
    + Send
    + Sync
    + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new().route(
        "/token",
        post(move |Form(form): Form<HashMap<String, String>>| {
            let handler = handler.clone();
            async move { handler(form) }
        }),
    );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (issuer, task)
}

#[test]
fn keys_preserve_exact_tuple_and_cannot_be_channel_hosts() {
    let base = target();
    let key = base.storage_key();
    assert!(key.starts_with("oauth-resource-v1:"));
    for (issuer, client, audience) in [
        (
            "https://issuer.example/",
            "rattler",
            "https://audit.example",
        ),
        ("https://issuer.example", "another", "https://audit.example"),
        (
            "https://issuer.example",
            "rattler",
            "https://audit.example/",
        ),
        ("https://issuer.example", "rattler", "https://AUDIT.example"),
    ] {
        assert_ne!(
            key,
            AudienceContext::new(issuer.into(), client.into(), audience.into())
                .unwrap()
                .storage_key()
        );
    }
    assert!(
        AudienceContext::new(
            "http://remote.example".into(),
            "rattler".into(),
            "audience".into()
        )
        .is_err()
    );
    assert!(
        AudienceContext::new(
            "https://issuer.example".into(),
            "rattler".into(),
            " ".into()
        )
        .is_err()
    );
}

#[tokio::test]
async fn channel_and_other_audience_credentials_are_untouched() {
    let target = target();
    let store = storage();
    let channel = Authentication::BearerToken("channel-fixture".into());
    store.store("issuer.example", &channel).unwrap();
    store.store("*.example", &channel).unwrap();
    assert_eq!(
        target
            .acquire(&store, ResourceInteraction::Deny, false, no_login)
            .await
            .unwrap_err(),
        ResourceOAuthError::AuthorizationRequired
    );
    let auth = credential("https://issuer.example/token", now() + 3600);
    let token = target
        .acquire(&store, ResourceInteraction::Allow, false, || async {
            Ok(auth.clone())
        })
        .await
        .unwrap();
    assert_eq!(token.access_token(), "fixture-access");
    assert!(!format!("{token:?}").contains("fixture-access"));
    assert_eq!(store.get("issuer.example").unwrap(), Some(channel.clone()));
    assert_eq!(
        store
            .get_by_url("https://issuer.example/channel")
            .unwrap()
            .1,
        Some(channel)
    );
    let other = AudienceContext::new(
        target.issuer.clone(),
        target.client_id.clone(),
        "other-audience".into(),
    )
    .unwrap();
    assert_eq!(
        other
            .acquire(&store, ResourceInteraction::Deny, false, no_login)
            .await
            .unwrap_err(),
        ResourceOAuthError::AuthorizationRequired
    );
    target
        .acquire(&store, ResourceInteraction::Deny, true, no_login)
        .await
        .unwrap();
}

#[tokio::test]
async fn cancelled_login_and_offline_never_replace_existing_grants() {
    let target = target();
    let store = storage();
    let mut old = credential("https://issuer.example/token", now() - 1);
    if let Authentication::OAuth { refresh_token, .. } = &mut old {
        *refresh_token = None;
    }
    store.write_resource(&target.storage_key(), &old).unwrap();
    assert_eq!(
        target
            .acquire(&store, ResourceInteraction::Allow, true, no_login)
            .await
            .unwrap_err(),
        ResourceOAuthError::AuthorizationRequired
    );
    assert_eq!(
        target
            .acquire(&store, ResourceInteraction::Allow, false, || async {
                Err(ResourceOAuthError::AuthorizationFailed)
            })
            .await
            .unwrap_err(),
        ResourceOAuthError::AuthorizationFailed
    );
    assert_eq!(
        store.read_resource(&target.storage_key()).unwrap(),
        Some(old)
    );
}

#[tokio::test]
async fn invalid_grant_metadata_is_not_persisted() {
    let target = target();
    let store = storage();
    for field in ["client", "expiry", "endpoint"] {
        let mut auth = credential("https://issuer.example/token", now() + 3600);
        if let Authentication::OAuth {
            client_id,
            expires_at,
            token_endpoint,
            ..
        } = &mut auth
        {
            match field {
                "client" => *client_id = "other".into(),
                "expiry" => *expires_at = None,
                "endpoint" => *token_endpoint = "http://untrusted.example/token".into(),
                _ => unreachable!(),
            }
        }
        assert_eq!(
            target
                .acquire(&store, ResourceInteraction::Allow, false, || async {
                    Ok(auth)
                })
                .await
                .unwrap_err(),
            ResourceOAuthError::InvalidCredential
        );
        assert!(
            store
                .read_resource(&target.storage_key())
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn concurrent_refresh_preserves_audience_and_rotated_token() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let (issuer, server)=endpoint(move |form| {
        count.fetch_add(1, Ordering::SeqCst);
        assert_eq!(form["audience"], "audit-resource"); assert_eq!(form["client_id"], "rattler");
        assert_eq!(form["refresh_token"], "fixture-refresh"); assert!(!form.contains_key("scope"));
        (StatusCode::OK, Json(json!({"access_token":"fresh-fixture", "refresh_token":"rotated-fixture", "expires_in":3600, "token_type":"Bearer"})))
    }).await;
    let target =
        AudienceContext::new(issuer.clone(), "rattler".into(), "audit-resource".into()).unwrap();
    let store = storage();
    store
        .write_resource(
            &target.storage_key(),
            &credential(&format!("{issuer}/token"), now() - 10),
        )
        .unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let target = target.clone();
        let store = store.clone();
        tasks.spawn(async move {
            target
                .acquire(&store, ResourceInteraction::Deny, false, no_login)
                .await
                .unwrap()
                .access_token()
                .to_owned()
        });
    }
    while let Some(result) = tasks.join_next().await {
        assert_eq!(result.unwrap(), "fresh-fixture");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        matches!(store.read_resource(&target.storage_key()).unwrap(), Some(Authentication::OAuth { refresh_token: Some(token), .. }) if token=="rotated-fixture")
    );
    server.abort();
}

#[tokio::test]
async fn transient_errors_never_prompt_and_provider_details_are_redacted() {
    let (issuer, server) = endpoint(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"fixture-secret", "error_description":"fixture-refresh"})),
        )
    })
    .await;
    let target =
        AudienceContext::new(issuer.clone(), "rattler".into(), "audit-resource".into()).unwrap();
    let store = storage();
    let old = credential(&format!("{issuer}/token"), now() - 10);
    store.write_resource(&target.storage_key(), &old).unwrap();
    let error = target
        .acquire(&store, ResourceInteraction::Allow, false, no_login)
        .await
        .unwrap_err();
    assert_eq!(error, ResourceOAuthError::RefreshFailed);
    assert!(!format!("{error:?} {error}").contains("fixture-secret"));
    assert_eq!(
        store.read_resource(&target.storage_key()).unwrap(),
        Some(old)
    );
    server.abort();
}

#[tokio::test]
async fn revoked_refresh_requires_interaction_without_touching_storage() {
    let (issuer, server) = endpoint(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_grant"})),
        )
    })
    .await;
    let target =
        AudienceContext::new(issuer.clone(), "rattler".into(), "audit-resource".into()).unwrap();
    let store = storage();
    let old = credential(&format!("{issuer}/token"), now() - 10);
    store.write_resource(&target.storage_key(), &old).unwrap();
    assert_eq!(
        target
            .acquire(&store, ResourceInteraction::Deny, false, no_login)
            .await
            .unwrap_err(),
        ResourceOAuthError::AuthorizationRequired
    );
    assert_eq!(
        store.read_resource(&target.storage_key()).unwrap(),
        Some(old)
    );
    server.abort();
}

#[derive(Debug)]
struct FailingBackend {
    auth: Option<Authentication>,
    fail_read: bool,
}
impl StorageBackend for FailingBackend {
    fn name(&self) -> String {
        "test-failure".into()
    }
    fn get(&self, _: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        if self.fail_read {
            Err(MemoryStorageError::LockError.into())
        } else {
            Ok(self.auth.clone())
        }
    }
    fn store(&self, _: &str, _: &Authentication) -> Result<(), AuthenticationStorageError> {
        Err(MemoryStorageError::LockError.into())
    }
    fn delete(&self, _: &str) -> Result<(), AuthenticationStorageError> {
        Err(MemoryStorageError::LockError.into())
    }
}

#[tokio::test]
async fn read_and_write_errors_fail_closed_without_shadowed_fallback() {
    let target = target();
    let auth = credential("https://issuer.example/token", now() + 3600);
    let mut unreadable = AuthenticationStorage::empty();
    unreadable.add_backend(Arc::new(FailingBackend {
        auth: None,
        fail_read: true,
    }));
    assert_eq!(
        target
            .acquire(&unreadable, ResourceInteraction::Allow, false, no_login)
            .await
            .unwrap_err(),
        ResourceOAuthError::Storage
    );
    let mut unwritable = AuthenticationStorage::empty();
    unwritable.add_backend(Arc::new(FailingBackend {
        auth: None,
        fail_read: false,
    }));
    assert_eq!(
        target
            .acquire(&unwritable, ResourceInteraction::Allow, false, || async {
                Ok(auth.clone())
            })
            .await
            .unwrap_err(),
        ResourceOAuthError::Storage
    );
    assert!(unwritable.get(&target.storage_key()).unwrap().is_none());
    let mut old = auth.clone();
    if let Authentication::OAuth {
        expires_at,
        refresh_token,
        ..
    } = &mut old
    {
        *expires_at = Some(now() - 1);
        *refresh_token = None;
    }
    let mut readonly = AuthenticationStorage::empty();
    readonly.add_backend(Arc::new(FailingBackend {
        auth: Some(old.clone()),
        fail_read: false,
    }));
    let fallback = Arc::new(MemoryStorage::new());
    readonly.add_backend(fallback.clone());
    assert_eq!(
        target
            .acquire(&readonly, ResourceInteraction::Allow, false, || async {
                Ok(auth)
            })
            .await
            .unwrap_err(),
        ResourceOAuthError::Storage
    );
    assert_eq!(
        readonly.read_resource(&target.storage_key()).unwrap(),
        Some(old)
    );
    assert!(fallback.get(&target.storage_key()).unwrap().is_none());
}

#[tokio::test]
async fn concurrent_missing_credentials_only_authorize_once() {
    let target = target();
    let store = storage();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let target = target.clone();
        let store = store.clone();
        let calls = calls.clone();
        tasks.spawn(async move {
            target
                .acquire(&store, ResourceInteraction::Allow, false, || async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    Ok(credential("https://issuer.example/token", now() + 3600))
                })
                .await
                .unwrap();
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
