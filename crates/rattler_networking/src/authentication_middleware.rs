//! `reqwest` middleware that authenticates requests with data from the
//! `AuthenticationStorage`
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use base64::{Engine, prelude::BASE64_STANDARD};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next};
use url::Url;

use crate::{
    Authentication, AuthenticationStorage, authentication_storage::AuthenticationStorageError,
    oauth_refresh,
};

/// `reqwest` middleware to authenticate requests
#[derive(Clone)]
pub struct AuthenticationMiddleware {
    auth_storage: AuthenticationStorage,
    // Explicit audience selection never falls back to host/wildcard credentials.
    oauth_audience: Option<(url::Origin, String)>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Middleware for AuthenticationMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        // If an `Authorization` header is already present, don't authenticate
        if req.headers().get(reqwest::header::AUTHORIZATION).is_some() {
            return next.run(req, extensions).await;
        }

        let url = req.url().clone();
        let selected = if let Some((origin, key)) = &self.oauth_audience {
            if &url.origin() != origin {
                return next.run(req, extensions).await;
            }
            self.auth_storage
                .get(key)
                .map(|auth| (url, auth.map(|auth| (key.clone(), auth))))
                .map_err(|_error| ())
        } else {
            self.auth_storage
                .get_by_url_with_host(url)
                .map_err(|_error| ())
        };
        match selected {
            Err(_) => {
                // Forward error to caller (invalid URL)
                next.run(req, extensions).await
            }
            Ok((url, auth_with_key)) => {
                // If this is an OAuth token, attempt refresh if expired
                let auth = match auth_with_key {
                    Some((matched_key, auth)) => {
                        let has_audience = matches!(
                            &auth,
                            Authentication::OAuth {
                                audience: Some(_),
                                ..
                            }
                        );
                        if has_audience != self.oauth_audience.is_some() {
                            return next.run(req, extensions).await;
                        }
                        let refresh_result = oauth_refresh::maybe_refresh_oauth(
                            &self.auth_storage,
                            auth,
                            &matched_key,
                        )
                        .await;
                        if let Some(failure) = refresh_result.failure() {
                            tracing::warn!(
                                "OAuth refresh for '{matched_key}' did not produce fresh credentials: {failure}"
                            );
                        }
                        refresh_result.into_authentication()
                    }
                    None => None,
                };

                let url = Self::authenticate_url(url, &auth);

                let mut req = req;
                *req.url_mut() = url;

                let req = Self::authenticate_request(req, &auth).await?;
                next.run(req, extensions).await
            }
        }
    }
}

impl AuthenticationMiddleware {
    /// Create a new authentication middleware with the given authentication
    /// storage
    pub fn from_auth_storage(auth_storage: AuthenticationStorage) -> Self {
        Self {
            auth_storage,
            oauth_audience: None,
        }
    }

    /// Create a new authentication middleware with the default authentication
    /// storage
    pub fn from_env_and_defaults() -> Result<Self, AuthenticationStorageError> {
        Ok(Self {
            auth_storage: AuthenticationStorage::from_env_and_defaults()?,
            oauth_audience: None,
        })
    }

    /// Select an exact audience grant, and send it only to `trusted_origin`.
    /// The origin is chosen by the caller, never inferred from the audience or
    /// a server challenge. No channel/wildcard fallback or interactive login.
    /// Use on a dedicated API client, not stacked with channel authentication.
    /// Disable redirects on the underlying client for credential-bearing requests.
    /// Audience refresh is supported on native targets only.
    pub fn with_oauth_audience(
        mut self,
        issuer: &str,
        client_id: &str,
        audience: &str,
        trusted_origin: url::Origin,
    ) -> Self {
        self.oauth_audience = Some((
            trusted_origin,
            AuthenticationStorage::oauth_audience_key(issuer, client_id, audience),
        ));
        self
    }

    /// Authenticate the given URL with the given authentication information
    fn authenticate_url(url: Url, auth: &Option<Authentication>) -> Url {
        if let Some(credentials) = auth {
            match credentials {
                Authentication::CondaToken(token) => {
                    let path = url.path();

                    let mut new_path = String::new();
                    new_path.push_str(format!("/t/{token}").as_str());
                    new_path.push_str(path);

                    let mut url = url.clone();
                    url.set_path(&new_path);
                    url
                }
                _ => url,
            }
        } else {
            url
        }
    }

    /// Authenticate the given request with the given authentication information
    async fn authenticate_request(
        mut req: reqwest::Request,
        auth: &Option<Authentication>,
    ) -> reqwest_middleware::Result<reqwest::Request> {
        if let Some(credentials) = auth {
            match credentials {
                Authentication::BearerToken(token) => {
                    let bearer_auth = format!("Bearer {token}");

                    let mut header_value = reqwest::header::HeaderValue::from_str(&bearer_auth)
                        .map_err(reqwest_middleware::Error::middleware)?;
                    header_value.set_sensitive(true);

                    req.headers_mut()
                        .insert(reqwest::header::AUTHORIZATION, header_value);
                    Ok(req)
                }
                Authentication::BasicHTTP { username, password } => {
                    let basic_auth = format!("{username}:{password}");
                    let basic_auth = BASE64_STANDARD.encode(basic_auth);
                    let basic_auth = format!("Basic {basic_auth}");

                    let mut header_value = reqwest::header::HeaderValue::from_str(&basic_auth)
                        .expect("base64 can always be converted to a header value");
                    header_value.set_sensitive(true);
                    req.headers_mut()
                        .insert(reqwest::header::AUTHORIZATION, header_value);
                    Ok(req)
                }
                Authentication::OAuth { access_token, .. } => {
                    let bearer_auth = format!("Bearer {access_token}");

                    let mut header_value = reqwest::header::HeaderValue::from_str(&bearer_auth)
                        .map_err(reqwest_middleware::Error::middleware)?;
                    header_value.set_sensitive(true);

                    req.headers_mut()
                        .insert(reqwest::header::AUTHORIZATION, header_value);
                    Ok(req)
                }
                Authentication::CondaToken(_) | Authentication::S3Credentials { .. } => Ok(req),
            }
        } else {
            Ok(req)
        }
    }
}

/// Returns the default auth storage directory used by rattler.
/// Would be placed in $HOME/.rattler, except when there is no home then it will
/// be put in '/rattler/'
pub fn default_auth_store_fallback_directory() -> &'static Path {
    static FALLBACK_AUTH_DIR: OnceLock<PathBuf> = OnceLock::new();
    FALLBACK_AUTH_DIR.get_or_init(|| {
        #[cfg(feature = "dirs")]
        return dirs::home_dir()
            .map_or_else(|| {
                tracing::warn!("using '/rattler' to store fallback authentication credentials because the home directory could not be found");
                // This can only happen if the dirs lib can't find a home directory this is very unlikely.
                PathBuf::from("/rattler/")
            }, |home| home.join(".rattler/"));
        #[cfg(not(feature = "dirs"))]
        {
            PathBuf::from("/rattler/")
        }
    })
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

    use axum::{
        Json, Router,
        extract::{Form, State},
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use futures::future::join_all;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::authentication_storage::backends::{file::FileStorage, memory::MemoryStorage};

    #[cfg(feature = "keyring")]
    // Requests are only authenticated when executed, so we need to capture and
    // cancel the request
    struct CaptureAbortMiddleware {
        pub captured_tx: tokio::sync::mpsc::Sender<reqwest::Request>,
    }

    #[cfg(feature = "keyring")]
    #[async_trait::async_trait]
    impl Middleware for CaptureAbortMiddleware {
        async fn handle(
            &self,
            req: Request,
            _: &mut http::Extensions,
            _: Next<'_>,
        ) -> reqwest_middleware::Result<Response> {
            self.captured_tx
                .send(req)
                .await
                .expect("failed to capture request");
            Err(reqwest_middleware::Error::middleware(
                std::io::Error::other("captured request, aborting"),
            ))
        }
    }

    #[cfg(feature = "keyring")]
    fn make_client_harness(
        storage: &AuthenticationStorage,
    ) -> (
        reqwest_middleware::ClientWithMiddleware,
        tokio::sync::mpsc::Receiver<reqwest::Request>,
    ) {
        let (captured_tx, captured_rx) = tokio::sync::mpsc::channel(1);
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::default())
            .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
                storage.clone(),
            )))
            .with_arc(Arc::new(CaptureAbortMiddleware { captured_tx }))
            .build();

        (client, captured_rx)
    }

    #[test]
    fn test_store_fallback() {
        let tdir = tempdir().unwrap();
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::from(
            FileStorage::from_path(tdir.path().to_path_buf().join("auth.json")).unwrap(),
        ));

        let host = "test.example.com";
        let authentication = Authentication::CondaToken("testtoken".to_string());
        storage.store(host, &authentication).unwrap();
        storage.delete(host).unwrap();
    }

    #[cfg(feature = "keyring")]
    #[tokio::test]
    async fn test_conda_token_storage() {
        let tdir = tempdir().unwrap();
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::from(
            FileStorage::from_path(tdir.path().to_path_buf().join("auth.json")).unwrap(),
        ));

        let host = "conda.example.com";

        let retrieved = storage.get(host);

        if let Err(e) = retrieved.as_ref() {
            println!("{e:?}");
        }

        assert!(retrieved.is_ok());
        assert!(retrieved.unwrap().is_none());

        let authentication = Authentication::CondaToken("testtoken".to_string());
        insta::assert_json_snapshot!(authentication, @r###"
        {
          "CondaToken": "testtoken"
        }
        "###);
        storage.store(host, &authentication).unwrap();

        let retrieved = storage.get(host);
        assert!(retrieved.is_ok());
        let retrieved = retrieved.unwrap();
        assert!(retrieved.is_some());
        let auth = retrieved.unwrap();
        assert!(auth == authentication);

        let (client, mut captured_rx) = make_client_harness(&storage);

        let request = client.get("https://conda.example.com/conda-forge/noarch/testpkg.tar.bz2");
        let request = request.build().unwrap();

        // we expect middleware error. if auth middleware fails, tests below will detect
        // it
        let _ = client.execute(request).await;

        let captured_request = captured_rx.recv().await.unwrap();
        assert!(captured_request.url().path().starts_with("/t/testtoken"));

        storage.delete(host).unwrap();
    }

    #[cfg(feature = "keyring")]
    #[tokio::test]
    async fn test_bearer_storage() {
        let tdir = tempdir().unwrap();
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::from(
            FileStorage::from_path(tdir.path().to_path_buf().join("auth.json")).unwrap(),
        ));
        let host = "bearer.example.com";

        let retrieved = storage.get(host);

        if let Err(e) = retrieved.as_ref() {
            println!("{e:?}");
        }

        assert!(retrieved.is_ok());
        assert!(retrieved.unwrap().is_none());

        let authentication = Authentication::BearerToken("xyztokytoken".to_string());

        insta::assert_json_snapshot!(authentication, @r###"
        {
          "BearerToken": "xyztokytoken"
        }
        "###);

        storage.store(host, &authentication).unwrap();

        let retrieved = storage.get(host);
        assert!(retrieved.is_ok());
        let retrieved = retrieved.unwrap();
        assert!(retrieved.is_some());
        let auth = retrieved.unwrap();
        assert!(auth == authentication);

        let (client, mut captured_rx) = make_client_harness(&storage);

        let request = client.get("https://bearer.example.com/conda-forge/noarch/testpkg.tar.bz2");
        let request = request.build().unwrap();
        let _ = client.execute(request).await;

        let captured_request = captured_rx.recv().await.unwrap();
        assert!(
            captured_request.url().to_string()
                == "https://bearer.example.com/conda-forge/noarch/testpkg.tar.bz2"
        );
        assert_eq!(
            captured_request.headers().get("Authorization").unwrap(),
            "Bearer xyztokytoken"
        );

        storage.delete(host).unwrap();
    }

    #[cfg(feature = "keyring")]
    #[tokio::test]
    async fn test_basic_auth_storage() {
        let tdir = tempdir().unwrap();
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::from(
            FileStorage::from_path(tdir.path().to_path_buf().join("auth.json")).unwrap(),
        ));
        let host = "basic.example.com";

        let retrieved = storage.get(host);

        if let Err(e) = retrieved.as_ref() {
            println!("{e:?}");
        }

        assert!(retrieved.is_ok());
        assert!(retrieved.unwrap().is_none());

        let authentication = Authentication::BasicHTTP {
            username: "testuser".to_string(),
            password: "testpassword".to_string(),
        };
        insta::assert_json_snapshot!(authentication, @r###"
        {
          "BasicHTTP": {
            "username": "testuser",
            "password": "testpassword"
          }
        }
        "###);
        storage.store(host, &authentication).unwrap();

        let retrieved = storage.get(host);
        assert!(retrieved.is_ok());
        let retrieved = retrieved.unwrap();
        assert!(retrieved.is_some());
        let auth = retrieved.unwrap();
        assert!(auth == authentication);

        let (client, mut captured_rx) = make_client_harness(&storage);

        let request = client.get("https://basic.example.com/conda-forge/noarch/testpkg.tar.bz2");
        let request = request.build().unwrap();
        let _ = client.execute(request).await;

        let captured_request = captured_rx.recv().await.unwrap();
        assert!(
            captured_request.url().to_string()
                == "https://basic.example.com/conda-forge/noarch/testpkg.tar.bz2"
        );
        assert_eq!(
            captured_request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .unwrap(),
            // this is the base64 encoding of "testuser:testpassword"
            "Basic dGVzdHVzZXI6dGVzdHBhc3N3b3Jk"
        );

        storage.delete(host).unwrap();
    }

    #[test]
    fn test_host_wildcard_expansion() {
        for (host, should_succeed) in [
            ("repo.prefix.dev", true),
            ("*.repo.prefix.dev", true),
            ("*.prefix.dev", true),
            ("*.dev", true),
            ("repo.notprefix.dev", false),
            ("*.repo.notprefix.dev", false),
            ("*.notprefix.dev", false),
            ("*.com", false),
        ] {
            let tdir = tempdir().unwrap();
            let mut storage = AuthenticationStorage::empty();
            storage.add_backend(Arc::from(
                FileStorage::from_path(tdir.path().to_path_buf().join("auth.json")).unwrap(),
            ));

            let authentication = Authentication::BearerToken("testtoken".to_string());

            storage.store(host, &authentication).unwrap();

            let retrieved = storage
                .get_by_url("https://repo.prefix.dev/conda-forge/noarch/repodata.json")
                .unwrap();

            if should_succeed {
                assert_eq!(retrieved.1, Some(authentication));
            } else {
                assert_eq!(retrieved.1, None);
            }
        }
    }

    #[tokio::test]
    async fn concurrent_oauth_refresh_is_coalesced_by_authentication_middleware() {
        #[derive(Clone)]
        struct TestState {
            audience: Option<&'static str>,
            refresh_count: Arc<AtomicUsize>,
            seen_authorization: Arc<Mutex<Vec<Option<String>>>>,
        }

        async fn token(
            State(state): State<TestState>,
            Form(form): Form<HashMap<String, String>>,
        ) -> (StatusCode, Json<serde_json::Value>) {
            assert_eq!(form.get("audience").map(String::as_str), state.audience);
            state.refresh_count.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(json!({
                    "access_token": "fresh-access-token",
                    "refresh_token": "rotated-refresh-token",
                    "expires_in": 3600,
                    "token_type": "Bearer",
                })),
            )
        }

        async fn repo(State(state): State<TestState>, headers: HeaderMap) -> &'static str {
            let authorization = headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned);
            state.seen_authorization.lock().unwrap().push(authorization);
            "ok"
        }

        for audience in [None, Some("https://audit.example")] {
            let state = TestState {
                audience,
                refresh_count: Arc::new(AtomicUsize::new(0)),
                seen_authorization: Arc::new(Mutex::new(Vec::new())),
            };
            let router = Router::new()
                .route("/token", post(token))
                .route("/repo", post(repo))
                .with_state(state.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

            let host = audience.map_or_else(
                || "127.0.0.1".to_owned(),
                |audience| {
                    AuthenticationStorage::oauth_audience_key(
                        "https://issuer.example",
                        "client-id",
                        audience,
                    )
                },
            );
            let mut storage = AuthenticationStorage::empty();
            storage.add_backend(Arc::new(MemoryStorage::new()));
            storage
                .store(
                    &host,
                    &Authentication::OAuth {
                        audience: audience.map(str::to_owned),
                        access_token: "expired-access-token".to_string(),
                        refresh_token: Some("refresh-token".to_string()),
                        expires_at: Some(0),
                        token_endpoint: format!("http://{addr}/token"),
                        revocation_endpoint: None,
                        client_id: "client-id".to_string(),
                    },
                )
                .unwrap();

            let repo_url = format!("http://{addr}/repo");
            let mut middleware = AuthenticationMiddleware::from_auth_storage(storage.clone());
            if let Some(audience) = audience {
                middleware = middleware.with_oauth_audience(
                    "https://issuer.example",
                    "client-id",
                    audience,
                    Url::parse(&repo_url).unwrap().origin(),
                );
            }
            let client = reqwest_middleware::ClientBuilder::new(
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
            )
            .with(middleware)
            .build();

            let responses = join_all((0..8).map(|_| client.post(&repo_url).send())).await;
            for response in responses {
                assert_eq!(response.unwrap().status(), StatusCode::OK);
            }

            assert_eq!(state.refresh_count.load(Ordering::SeqCst), 1);
            assert!(
                matches!(storage.get(&host).unwrap(), Some(Authentication::OAuth { audience: saved, refresh_token: Some(refresh), .. }) if saved.as_deref() == audience && refresh == "rotated-refresh-token")
            );
            let seen_authorization = state.seen_authorization.lock().unwrap();
            assert_eq!(seen_authorization.len(), 8);
            assert!(
                seen_authorization
                    .iter()
                    .all(|auth| { auth.as_deref() == Some("Bearer fresh-access-token") })
            );
        }
    }

    #[tokio::test]
    async fn expired_oauth_with_failed_refresh_sends_no_authorization_header() {
        #[derive(Clone)]
        struct TestState {
            seen_authorization: Arc<Mutex<Vec<Option<String>>>>,
        }

        // A rotating server that has already invalidated this refresh token.
        async fn token() -> (StatusCode, Json<serde_json::Value>) {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_grant" })),
            )
        }

        async fn repo(State(state): State<TestState>, headers: HeaderMap) -> &'static str {
            let authorization = headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned);
            state.seen_authorization.lock().unwrap().push(authorization);
            "ok"
        }

        let state = TestState {
            seen_authorization: Arc::new(Mutex::new(Vec::new())),
        };
        let router = Router::new()
            .route("/token", post(token))
            .route("/repo", post(repo))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let host = "127.0.0.1";
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        storage
            .store(
                host,
                &Authentication::OAuth {
                    audience: None,
                    access_token: "expired-access-token".to_string(),
                    refresh_token: Some("refresh-token".to_string()),
                    expires_at: Some(0),
                    token_endpoint: format!("http://{addr}/token"),
                    revocation_endpoint: None,
                    client_id: "client-id".to_string(),
                },
            )
            .unwrap();

        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::default())
            .with(AuthenticationMiddleware::from_auth_storage(storage))
            .build();

        let response = client
            .post(format!("http://{addr}/repo"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Refresh failed and the access token is expired, so no expired bearer
        // token should leak to the backend.
        let seen_authorization = state.seen_authorization.lock().unwrap();
        assert_eq!(seen_authorization.as_slice(), &[None]);
    }

    #[test]
    fn test_rattler_auth_file_env_var_handling() {
        let tdir = tempdir().unwrap();

        let storage = temp_env::with_var(
            "RATTLER_AUTH_FILE",
            Some(
                tdir.path()
                    .to_path_buf()
                    .join("auth.json")
                    .to_str()
                    .unwrap(),
            ),
            || AuthenticationStorage::from_env_and_defaults().unwrap(),
        );

        let host = "test.example.com";
        let authentication = Authentication::CondaToken("testtoken".to_string());
        storage.store(host, &authentication).unwrap();

        let file = tdir.path().join("auth.json");
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            "{\"test.example.com\":{\"CondaToken\":\"testtoken\"}}"
        );
    }

    #[tokio::test]
    async fn audience_credentials_require_exact_context_and_origin() {
        async fn authorization(
            client: &reqwest_middleware::ClientWithMiddleware,
            url: Url,
        ) -> String {
            client.post(url).send().await.unwrap().text().await.unwrap()
        }
        const ISSUER: &str = "https://issuer.example";
        const AUDIENCE: &str = "https://audit.example";
        let router = Router::new().route(
            "/api",
            post(|headers: HeaderMap| async move {
                headers
                    .get("authorization")
                    .map(|header| header.to_str().unwrap().to_owned())
                    .unwrap_or_default()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/api", listener.local_addr().unwrap())).unwrap();
        let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_url = Url::parse(&format!("http://{}/api", other.local_addr().unwrap())).unwrap();
        let second_router = router.clone();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let other_server =
            tokio::spawn(async move { axum::serve(other, second_router).await.unwrap() });
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        let auth = Authentication::OAuth {
            audience: Some(AUDIENCE.into()),
            access_token: "fixture.opaque.token".into(),
            refresh_token: None,
            expires_at: Some(i64::MAX),
            token_endpoint: "https://issuer.example/token".into(),
            revocation_endpoint: None,
            client_id: "rattler".into(),
        };
        storage
            .store(
                &AuthenticationStorage::oauth_audience_key(ISSUER, "rattler", AUDIENCE),
                &auth,
            )
            .unwrap();
        let channel = Authentication::BearerToken("fixture-channel".into());
        storage.store("127.0.0.1", &channel).unwrap();
        let client =
            |issuer, id, audience| {
                reqwest_middleware::ClientBuilder::new(
                    reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .unwrap(),
                )
                .with(
                    AuthenticationMiddleware::from_auth_storage(storage.clone())
                        .with_oauth_audience(issuer, id, audience, url.origin()),
                )
                .build()
            };
        let http = client(ISSUER, "rattler", AUDIENCE);
        assert_eq!(
            authorization(&http, url.clone()).await,
            "Bearer fixture.opaque.token"
        );
        assert_eq!(storage.get("127.0.0.1").unwrap(), Some(channel));
        assert_eq!(authorization(&http, other_url).await, "");
        // Even a misplaced audience grant under a host key must not be a fallback.
        storage.store("127.0.0.1", &auth).unwrap();
        for (issuer, id, audience) in [
            ("https://other.example", "rattler", AUDIENCE),
            (ISSUER, "other", AUDIENCE),
            (ISSUER, "rattler", "other-audience"),
        ] {
            assert_eq!(
                authorization(&client(issuer, id, audience), url.clone()).await,
                ""
            );
        }
        let channel_http = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(AuthenticationMiddleware::from_auth_storage(storage))
            .build();
        assert_eq!(authorization(&channel_http, url).await, "");
        server.abort();
        other_server.abort();
    }
}
