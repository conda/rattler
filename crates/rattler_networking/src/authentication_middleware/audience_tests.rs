use super::*;
use crate::authentication_storage::{
    StorageBackend,
    backends::{file::FileStorage, memory::MemoryStorage},
};
use axum::{
    Json, Router,
    extract::Form,
    http::HeaderMap,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const ISSUER: &str = "https://issuer.example";
const AUDIENCE: &str = "https://audit.example";
fn key() -> String {
    AuthenticationStorage::oauth_audience_key(ISSUER, "rattler", AUDIENCE)
}
fn grant(endpoint: &str) -> Authentication {
    Authentication::OAuth {
        audience: Some(AUDIENCE.into()),
        access_token: "fixture.old.token".into(),
        refresh_token: Some("fixture-refresh".into()),
        expires_at: Some(0),
        token_endpoint: endpoint.into(),
        revocation_endpoint: None,
        client_id: "rattler".into(),
    }
}
fn storage() -> AuthenticationStorage {
    let mut storage = AuthenticationStorage::empty();
    storage.add_backend(Arc::new(MemoryStorage::new()));
    storage
}
type Forms = Arc<Mutex<Vec<HashMap<String, String>>>>;
async fn server(response: Value) -> (Url, Forms, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let forms = Forms::default();
    let captured = forms.clone();
    let router = Router::new()
        .route(
            "/api",
            get(|headers: HeaderMap| async move {
                headers
                    .get("authorization")
                    .map(|header| header.to_str().unwrap().to_owned())
                    .unwrap_or_default()
            }),
        )
        .route(
            "/token",
            post(move |Form(form): Form<HashMap<String, String>>| {
                captured.lock().unwrap().push(form);
                let response = response.clone();
                async move { Json(response) }
            }),
        );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, forms, task)
}
fn client(
    storage: AuthenticationStorage,
    issuer: &str,
    client_id: &str,
    audience: &str,
    url: &Url,
) -> reqwest_middleware::ClientWithMiddleware {
    reqwest_middleware::ClientBuilder::new(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
    )
    .with(
        AuthenticationMiddleware::from_auth_storage(storage).with_oauth_audience(
            issuer,
            client_id,
            audience,
            url.origin(),
        ),
    )
    .build()
}

#[test]
fn keys_are_exact_and_existing_credentials_deserialize_unchanged() {
    let base = key();
    for (issuer, client, audience) in [
        ("https://ISSUER.example", "rattler", AUDIENCE),
        (ISSUER, "other-client", AUDIENCE),
        (ISSUER, "rattler", "https://audit.example/"),
    ] {
        assert_ne!(
            base,
            AuthenticationStorage::oauth_audience_key(issuer, client, audience)
        );
    }
    let mut old = serde_json::to_value(grant("https://issuer.example/token")).unwrap();
    old["OAuth"].as_object_mut().unwrap().remove("audience");
    let auth: Authentication = serde_json::from_value(old.clone()).unwrap();
    assert!(matches!(
        &auth,
        Authentication::OAuth { audience: None, .. }
    ));
    assert_eq!(serde_json::to_value(auth).unwrap(), old);
}

#[tokio::test]
async fn caller_stores_grant_and_existing_middleware_refreshes_only_for_exact_origin_and_context() {
    let (url, forms, task) = server(json!({"access_token":"fixture.new.token", "refresh_token":"fixture-rotated", "expires_in":3600,"token_type":"Bearer"})).await;
    let store = storage();
    let channel = Authentication::BearerToken("fixture-channel".into());
    store.store("127.0.0.1", &channel).unwrap();
    store
        .store(&key(), &grant(url.join("token").unwrap().as_str()))
        .unwrap();
    let http = client(store.clone(), ISSUER, "rattler", AUDIENCE, &url);
    let requests = (0..8).map(|_| http.get(url.join("api").unwrap()).send());
    for response in futures::future::join_all(requests).await {
        assert_eq!(
            response.unwrap().text().await.unwrap(),
            "Bearer fixture.new.token"
        );
    }
    assert_eq!(forms.lock().unwrap().len(), 1);
    let form = forms.lock().unwrap()[0].clone();
    assert_eq!(form["audience"], AUDIENCE);
    assert_eq!(form["client_id"], "rattler");
    assert_eq!(form["refresh_token"], "fixture-refresh");
    assert_eq!(store.get("127.0.0.1").unwrap(), Some(channel));
    assert!(
        matches!(store.get(&key()).unwrap(), Some(Authentication::OAuth { audience: Some(aud), refresh_token: Some(refresh), .. }) if aud == AUDIENCE && refresh == "fixture-rotated")
    );
    // Even an audience grant under a host key is not a fallback for an absent tuple.
    store
        .store("127.0.0.1", &store.get(&key()).unwrap().unwrap())
        .unwrap();
    for (issuer, id, audience) in [
        ("https://other.example", "rattler", AUDIENCE),
        (ISSUER, "other", AUDIENCE),
        (ISSUER, "rattler", "other-audience"),
    ] {
        let http = client(store.clone(), issuer, id, audience, &url);
        assert_eq!(
            http.get(url.join("api").unwrap())
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            ""
        );
    }
    let (other_url, _, other_task) = server(json!({})).await;
    assert_eq!(
        http.get(other_url.join("api").unwrap())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        ""
    );
    // Ordinary channel middleware must also ignore that misplaced audience grant.
    let channel_http = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with(AuthenticationMiddleware::from_auth_storage(store))
        .build();
    assert_eq!(
        channel_http
            .get(url.join("api").unwrap())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        ""
    );
    task.abort();
    other_task.abort();
}

#[tokio::test]
async fn refresh_preserves_omitted_refresh_token_and_rejects_invalid_responses() {
    for (response, valid) in [
        (
            json!({"access_token":"fixture.fresh.token", "expires_in":3600,"token_type":"bearer"}),
            true,
        ),
        (
            json!({"access_token":"fixture", "expires_in":3600,"token_type":"MAC"}),
            false,
        ),
        (
            json!({"access_token":"fixture", "token_type":"Bearer"}),
            false,
        ),
        (
            json!({"access_token":"fixture", "expires_in":-1,"token_type":"Bearer"}),
            false,
        ),
        (
            json!({"access_token":"fixture", "expires_in":i64::MAX,"token_type":"Bearer"}),
            false,
        ),
        (
            json!({"access_token":"fixture", "expires_in":3600,"token_type":"Bearer","refresh_token":""}),
            false,
        ),
        (json!("x".repeat(70_000)), false),
    ] {
        let (url, _, task) = server(response).await;
        let store = storage();
        let old = grant(url.join("token").unwrap().as_str());
        store.store(&key(), &old).unwrap();
        let result = oauth_refresh::maybe_refresh_oauth(&store, old.clone(), &key()).await;
        if valid {
            assert!(result.failure().is_none());
            assert!(
                matches!(result.into_authentication(), Some(Authentication::OAuth { refresh_token: Some(refresh), audience: Some(aud), .. }) if refresh == "fixture-refresh" && aud == AUDIENCE)
            );
        } else {
            assert!(result.failure().is_some());
            assert!(result.into_authentication().is_none());
            assert_eq!(store.get(&key()).unwrap(), Some(old));
        }
        task.abort();
    }
}

#[tokio::test]
async fn audience_refresh_does_not_follow_redirects() {
    use axum::http::{StatusCode, header::LOCATION};
    let (destination, forms, target) =
        server(json!({"access_token":"fixture", "expires_in":3600,"token_type":"Bearer"})).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
    let redirect = destination.join("token").unwrap().to_string();
    let router = Router::new().route(
        "/token",
        post(move || {
            let redirect = redirect.clone();
            async move { (StatusCode::TEMPORARY_REDIRECT, [(LOCATION, redirect)]) }
        }),
    );
    let source = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let store = storage();
    let old = grant(&endpoint);
    store.store(&key(), &old).unwrap();
    let result = oauth_refresh::maybe_refresh_oauth(&store, old.clone(), &key()).await;
    assert!(result.failure().is_some());
    assert!(result.into_authentication().is_none());
    assert!(forms.lock().unwrap().is_empty());
    assert_eq!(store.get(&key()).unwrap(), Some(old));
    source.abort();
    target.abort();
}

#[derive(Debug)]
struct FailedBackend {
    old: Authentication,
    fail_read: bool,
}
fn storage_error() -> AuthenticationStorageError {
    AuthenticationStorageError::StoreFailed {
        host: "fixture".into(),
        backends: "fixture".into(),
    }
}
impl StorageBackend for FailedBackend {
    fn name(&self) -> String {
        "fixture".into()
    }
    fn get(&self, _key: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        if self.fail_read {
            Err(storage_error())
        } else {
            Ok(Some(self.old.clone()))
        }
    }
    fn store(&self, _key: &str, _auth: &Authentication) -> Result<(), AuthenticationStorageError> {
        Err(storage_error())
    }
    fn delete(&self, _key: &str) -> Result<(), AuthenticationStorageError> {
        Err(storage_error())
    }
}

#[tokio::test]
async fn storage_failure_never_returns_or_shadows_unpersisted_refresh() {
    let (url, forms, task) =
        server(json!({"access_token":"fixture-new", "expires_in":3600,"token_type":"Bearer"}))
            .await;
    for fail_read in [true, false] {
        let mut store = AuthenticationStorage::empty();
        let old = grant(url.join("token").unwrap().as_str());
        store.add_backend(Arc::new(FailedBackend {
            old: old.clone(),
            fail_read,
        }));
        let fallback = Arc::new(MemoryStorage::new());
        store.add_backend(fallback.clone());
        let result = oauth_refresh::maybe_refresh_oauth(&store, old.clone(), &key()).await;
        assert!(result.failure().is_some());
        assert!(result.into_authentication().is_none());
        assert_eq!(fallback.get(&key()).unwrap(), None);
        if !fail_read {
            assert_eq!(store.get(&key()).unwrap(), Some(old));
        }
    }
    assert_eq!(forms.lock().unwrap().len(), 1);
    task.abort();
}

#[test]
fn concurrent_file_updates_preserve_audience_and_channel_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("credentials.json");
    let file = Arc::new(FileStorage::from_path(path.clone()).unwrap());
    let mut store = AuthenticationStorage::empty();
    store.add_backend(file);
    store
        .store("delete-me", &Authentication::BearerToken("fixture".into()))
        .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(11));
    let mut jobs = Vec::new();
    for i in 0..10 {
        let store = store.clone();
        let barrier = barrier.clone();
        jobs.push(std::thread::spawn(move || {
            barrier.wait();
            match i {
                8 => store
                    .store(
                        "channel.example",
                        &Authentication::BearerToken("fixture-channel".into()),
                    )
                    .unwrap(),
                9 => store.delete("delete-me").unwrap(),
                _ => {
                    let audience = format!("audience-{i}");
                    let mut auth = grant("https://issuer.example/token");
                    if let Authentication::OAuth {
                        audience: value, ..
                    } = &mut auth
                    {
                        *value = Some(audience.clone());
                    }
                    store
                        .store(
                            &AuthenticationStorage::oauth_audience_key(
                                ISSUER, "rattler", &audience,
                            ),
                            &auth,
                        )
                        .unwrap();
                }
            }
        }));
    }
    barrier.wait();
    for job in jobs {
        job.join().unwrap();
    }
    let reopened = FileStorage::from_path(path).unwrap();
    assert_eq!(reopened.list().unwrap().len(), 9);
    assert!(reopened.get("delete-me").unwrap().is_none());
    assert!(reopened.get("channel.example").unwrap().is_some());
    for i in 0..8 {
        assert!(
            reopened
                .get(&AuthenticationStorage::oauth_audience_key(
                    ISSUER,
                    "rattler",
                    &format!("audience-{i}")
                ))
                .unwrap()
                .is_some()
        );
    }
}
