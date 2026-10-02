use super::*;

#[tokio::test]
async fn dropped_interaction_preserves_grant_and_releases_gate() {
    let resource = target();
    let store = storage();
    let mut old = credential("https://issuer.example/token", now() - 10);
    if let Authentication::OAuth { refresh_token, .. } = &mut old {
        *refresh_token = None;
    }
    store.write_resource(&resource.storage_key(), &old).unwrap();
    let result = tokio::time::timeout(
        Duration::from_millis(10),
        resource.acquire(
            &store,
            ResourceInteraction::Allow,
            false,
            std::future::pending,
        ),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        store.read_resource(&resource.storage_key()).unwrap(),
        Some(old)
    );
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            resource.acquire(&store, ResourceInteraction::Deny, false, no_login)
        )
        .await
        .unwrap()
        .unwrap_err(),
        ResourceOAuthError::AuthorizationRequired
    );
}

#[tokio::test]
async fn forced_refresh_preserves_omitted_refresh_token() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let (issuer, server) = endpoint(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        (
            StatusCode::OK,
            Json(json!({"access_token":"fresh-fixture", "token_type":"bearer", "expires_in":3600})),
        )
    })
    .await;
    let resource = AudienceContext::new(issuer.clone(), "rattler".into(), "audit".into()).unwrap();
    let store = storage();
    store
        .write_resource(
            &resource.storage_key(),
            &credential(&format!("{issuer}/token"), now() + 3600),
        )
        .unwrap();
    assert_eq!(
        refresh_audience(&issuer, "rattler", "audit", &store)
            .await
            .unwrap()
            .access_token(),
        "fresh-fixture"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        matches!(store.read_resource(&resource.storage_key()).unwrap(),Some(Authentication::OAuth {refresh_token:Some(token), ..}) if token=="fixture-refresh")
    );
    server.abort();
}

#[tokio::test]
async fn refresh_persistence_failure_retains_old_record_without_caching_new_token() {
    let (issuer,server)=endpoint(|_| (StatusCode::OK,Json(json!({"access_token":"fresh-fixture", "refresh_token":"rotated-fixture", "token_type":"Bearer", "expires_in":3600})))).await;
    let resource = AudienceContext::new(issuer.clone(), "rattler".into(), "audit".into()).unwrap();
    let old = credential(&format!("{issuer}/token"), now() - 10);
    let mut store = AuthenticationStorage::empty();
    store.add_backend(Arc::new(FailingBackend {
        auth: Some(old.clone()),
        fail_read: false,
    }));
    assert_eq!(
        resource.refresh_cached(&store).await.unwrap_err(),
        ResourceOAuthError::Storage
    );
    assert_eq!(store.get(&resource.storage_key()).unwrap(), Some(old));
    server.abort();
}

#[tokio::test]
async fn malformed_refresh_responses_preserve_existing_grant() {
    for response in [
        json!({"access_token":"fixture", "token_type":"MAC", "expires_in":3600}),
        json!({"access_token":"fixture", "token_type":"Bearer", "expires_in":-1}),
        json!({"access_token":"fixture", "token_type":"Bearer", "expires_in":i64::MAX}),
        json!({"access_token":"fixture", "token_type":"Bearer", "expires_in":3600,"refresh_token":""}),
        json!({"access_token":"fixture\r\nsecret", "token_type":"Bearer", "expires_in":3600}),
        json!({"access_token":"fixture", "token_type":"Bearer"}),
    ] {
        let (issuer, server) = endpoint(move |_| (StatusCode::OK, Json(response.clone()))).await;
        let resource =
            AudienceContext::new(issuer.clone(), "rattler".into(), "audit".into()).unwrap();
        let store = storage();
        let old = credential(&format!("{issuer}/token"), now() - 10);
        store.write_resource(&resource.storage_key(), &old).unwrap();
        assert!(resource.refresh_cached(&store).await.is_err());
        assert_eq!(
            store.read_resource(&resource.storage_key()).unwrap(),
            Some(old)
        );
        server.abort();
    }
}

#[tokio::test]
async fn refresh_redirect_does_not_forward_credentials() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app = Router::new()
        .route(
            "/token",
            post(|| async { axum::response::Redirect::temporary("/sink") }),
        )
        .route(
            "/sink",
            post(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let resource = AudienceContext::new(issuer.clone(), "rattler".into(), "audit".into()).unwrap();
    let store = storage();
    let old = credential(&format!("{issuer}/token"), now() - 10);
    store.write_resource(&resource.storage_key(), &old).unwrap();
    assert_eq!(
        resource.refresh_cached(&store).await.unwrap_err(),
        ResourceOAuthError::RefreshFailed
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.read_resource(&resource.storage_key()).unwrap(),
        Some(old)
    );
    server.abort();
}

#[tokio::test]
async fn tokens_are_opaque_even_when_they_contain_dots() {
    let resource = target();
    let store = storage();
    let expires = now() + 3600;
    let mut auth = credential("https://issuer.example/token", expires);
    if let Authentication::OAuth { access_token, .. } = &mut auth {
        *access_token = "opaque.token.value".into();
    }
    assert_eq!(resource.validate(&auth).unwrap(), expires);
    let token = resource
        .acquire(&store, ResourceInteraction::Allow, false, || async {
            Ok(auth)
        })
        .await
        .unwrap();
    assert_eq!(token.access_token(), "opaque.token.value");
    let cached = resource
        .acquire(&store, ResourceInteraction::Deny, true, no_login)
        .await
        .unwrap();
    assert_eq!(cached.access_token(), "opaque.token.value");
}

#[test]
fn different_audiences_and_channel_updates_share_one_file_transaction_lock() {
    use crate::authentication_storage::backends::file::FileStorage;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("credentials.json");
    let file = Arc::new(FileStorage::from_path(path.clone()).unwrap());
    let channel = Authentication::CondaToken("fixture-channel".into());
    file.store("issuer.example", &channel).unwrap();
    file.store("obsolete-channel", &channel).unwrap();
    let mut storage = AuthenticationStorage::empty();
    storage.add_backend(file.clone());
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let mut threads = Vec::new();
    for i in 0..8 {
        let storage = storage.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let resource = AudienceContext::new(
                        "https://issuer.example".into(),
                        "rattler".into(),
                        format!("audience-{i}"),
                    )
                    .unwrap();
                    resource
                        .acquire(&storage, ResourceInteraction::Allow, false, || async {
                            barrier.wait();
                            Ok(credential("https://issuer.example/token", now() + 3600))
                        })
                        .await
                        .unwrap();
                });
        }));
    }
    threads.push(std::thread::spawn(move || {
        barrier.wait();
        file.delete("obsolete-channel").unwrap();
    }));
    for thread in threads {
        thread.join().unwrap();
    }
    let reopened = FileStorage::from_path(path).unwrap();
    assert_eq!(reopened.list().unwrap().len(), 9);
    assert_eq!(reopened.get("issuer.example").unwrap(), Some(channel));
    assert!(reopened.get("obsolete-channel").unwrap().is_none());
    for i in 0..8 {
        let resource = AudienceContext::new(
            "https://issuer.example".into(),
            "rattler".into(),
            format!("audience-{i}"),
        )
        .unwrap();
        assert!(reopened.get(&resource.storage_key()).unwrap().is_some());
    }
}
