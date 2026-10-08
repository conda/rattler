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
    // A pinned storage key, sent only to the trusted origin. Pinning never
    // falls back to host/wildcard credentials.
    pinned_credential: Option<(url::Origin, String)>,
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
        let selected = if let Some((origin, key)) = &self.pinned_credential {
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
                // If this is an OAuth token, attempt refresh if expired. The
                // storage key is the policy: a grant stored under a host is
                // sent to that host whether or not it carries an audience, and
                // a pinned key is sent to its trusted origin as-is.
                let auth = match auth_with_key {
                    Some((matched_key, auth)) => {
                        if self.pinned_credential.is_some()
                            && !matches!(
                                auth,
                                Authentication::OAuth { .. } | Authentication::BearerToken(_)
                            )
                        {
                            // A pinned key only ever becomes a bearer header on
                            // the foreign origin; never splice other credential
                            // kinds (conda tokens, basic auth, S3) into it.
                            tracing::warn!(
                                "Credential stored under '{matched_key}' is not a bearer-style grant; not sending it to the pinned origin"
                            );
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
            pinned_credential: None,
        }
    }

    /// Create a new authentication middleware with the default authentication
    /// storage
    pub fn from_env_and_defaults() -> Result<Self, AuthenticationStorageError> {
        Ok(Self {
            auth_storage: AuthenticationStorage::from_env_and_defaults()?,
            pinned_credential: None,
        })
    }

    /// Send the credential stored under exactly `key`, and only to
    /// `trusted_origin`. Requests to any other origin are left anonymous.
    ///
    /// This lets a grant obtained for one host (e.g. `prefix.dev`, whose
    /// login requests an audience for a sibling API) be used against that
    /// API's origin, which host/wildcard lookup would never resolve to.
    /// The origin is chosen by the caller, never inferred from the key or a
    /// server challenge. No channel/wildcard fallback or interactive login.
    /// Use on a dedicated API client, not stacked with channel authentication.
    /// Disable redirects on the underlying client for credential-bearing requests.
    /// OAuth refresh is supported on native targets only.
    pub fn with_credential_key(
        mut self,
        key: impl Into<String>,
        trusted_origin: url::Origin,
    ) -> Self {
        self.pinned_credential = Some((trusted_origin, key.into()));
        self
    }

    /// Select an exact audience grant stored under
    /// [`AuthenticationStorage::oauth_audience_key`], and send it only to
    /// `trusted_origin`. See [`Self::with_credential_key`] for the policy.
    ///
    /// Storing a grant under a separate audience key next to its host entry
    /// creates two copies of one rotating refresh token; the login CLI stores
    /// each grant once, under its host, so prefer pinning that host key.
    #[deprecated(
        note = "store the grant once under its login host and pin that key with `with_credential_key`"
    )]
    pub fn with_oauth_audience(
        self,
        issuer: &str,
        client_id: &str,
        audience: &str,
        trusted_origin: url::Origin,
    ) -> Self {
        self.with_credential_key(
            AuthenticationStorage::oauth_audience_key(issuer, client_id, audience),
            trusted_origin,
        )
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

    // Requests are only authenticated when executed, so we need to capture and
    // cancel the request
    struct CaptureAbortMiddleware {
        pub captured_tx: tokio::sync::mpsc::Sender<reqwest::Request>,
    }

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

    fn make_client_harness(
        middleware: AuthenticationMiddleware,
    ) -> (
        reqwest_middleware::ClientWithMiddleware,
        tokio::sync::mpsc::Receiver<reqwest::Request>,
    ) {
        let (captured_tx, captured_rx) = tokio::sync::mpsc::channel(1);
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::default())
            .with(middleware)
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

        let (client, mut captured_rx) =
            make_client_harness(AuthenticationMiddleware::from_auth_storage(storage.clone()));

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

        let (client, mut captured_rx) =
            make_client_harness(AuthenticationMiddleware::from_auth_storage(storage.clone()));

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

        let (client, mut captured_rx) =
            make_client_harness(AuthenticationMiddleware::from_auth_storage(storage.clone()));

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
    #[allow(deprecated)]
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
    #[allow(deprecated)]
    async fn audience_credentials_require_exact_context_and_origin() {
        const ISSUER: &str = "https://issuer.example";
        const AUDIENCE: &str = "https://audit.example";
        let origin = Url::parse(AUDIENCE).unwrap().origin();
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
        storage
            .store(
                "audit.example",
                &Authentication::BearerToken("fixture-channel".into()),
            )
            .unwrap();
        for (issuer, client_id, audience, url, expected) in [
            (
                ISSUER,
                "rattler",
                AUDIENCE,
                AUDIENCE,
                Some("Bearer fixture.opaque.token"),
            ),
            (ISSUER, "rattler", AUDIENCE, "https://other.example", None),
            (
                ISSUER,
                "rattler",
                AUDIENCE,
                "https://audit.example:444",
                None,
            ),
            (ISSUER, "rattler", AUDIENCE, "http://audit.example", None),
            ("https://other.example", "rattler", AUDIENCE, AUDIENCE, None),
            (ISSUER, "other", AUDIENCE, AUDIENCE, None),
            (ISSUER, "rattler", "other-audience", AUDIENCE, None),
        ] {
            let middleware = AuthenticationMiddleware::from_auth_storage(storage.clone())
                .with_oauth_audience(issuer, client_id, audience, origin.clone());
            let (http, mut captured) = make_client_harness(middleware);
            let _ = http.post(url).send().await;
            let request = captured.recv().await.unwrap();
            assert_eq!(
                request
                    .headers()
                    .get("authorization")
                    .map(|v| v.to_str().unwrap()),
                expected
            );
        }
        // A grant stored under a host is sent to that host, audience or not:
        // the storage key is the policy.
        storage.store("audit.example", &auth).unwrap();
        let (http, mut captured) =
            make_client_harness(AuthenticationMiddleware::from_auth_storage(storage));
        let _ = http.post(AUDIENCE).send().await;
        assert_eq!(
            captured
                .recv()
                .await
                .unwrap()
                .headers()
                .get("authorization")
                .map(|v| v.to_str().unwrap()),
            Some("Bearer fixture.opaque.token")
        );
    }

    #[tokio::test]
    async fn pinned_credential_key_is_sent_only_to_trusted_origin() {
        // `pixi auth login prefix.dev` stores one grant under the host key and
        // requests an audience for the Basilisk API. The channel middleware
        // keeps using it for prefix.dev; an API client pins the same key to
        // the API origin, which host lookup would never resolve to.
        const API: &str = "https://api.basilisk.example";
        let api_origin = Url::parse(API).unwrap().origin();
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        let grant = Authentication::OAuth {
            audience: Some(API.into()),
            access_token: "fixture.host.token".into(),
            refresh_token: None,
            expires_at: Some(i64::MAX),
            token_endpoint: "https://issuer.example/token".into(),
            revocation_endpoint: None,
            client_id: "rattler".into(),
        };
        storage.store("issuer.example", &grant).unwrap();
        storage
            .store(
                "*.basilisk.example",
                &Authentication::BearerToken("fixture-wildcard".into()),
            )
            .unwrap();

        let header = |request: &Request| {
            request
                .headers()
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_owned())
        };

        // Channel middleware: host lookup finds the grant for the issuer host
        // and the wildcard for the API host; neither is cross-wired.
        let (http, mut captured) =
            make_client_harness(AuthenticationMiddleware::from_auth_storage(storage.clone()));
        let _ = http.post("https://issuer.example/channel").send().await;
        assert_eq!(
            header(&captured.recv().await.unwrap()).as_deref(),
            Some("Bearer fixture.host.token")
        );
        let _ = http.post(API).send().await;
        assert_eq!(
            header(&captured.recv().await.unwrap()).as_deref(),
            Some("Bearer fixture-wildcard")
        );

        // Pinned middleware: the host grant goes to the API origin only.
        let pinned = AuthenticationMiddleware::from_auth_storage(storage.clone())
            .with_credential_key("issuer.example", api_origin.clone());
        for (url, expected) in [
            (format!("{API}/v1/audit"), Some("Bearer fixture.host.token")),
            ("https://issuer.example/channel".to_string(), None),
            ("https://api.basilisk.example:444/".to_string(), None),
            ("http://api.basilisk.example/".to_string(), None),
        ] {
            let (http, mut captured) = make_client_harness(pinned.clone());
            let _ = http.post(&url).send().await;
            assert_eq!(
                header(&captured.recv().await.unwrap()).as_deref(),
                expected,
                "{url}"
            );
        }

        // A pinned key only ever becomes a bearer header: other credential
        // kinds are not spliced into the foreign origin's URL or headers.
        storage
            .store(
                "conda.example",
                &Authentication::CondaToken("fixture-conda".into()),
            )
            .unwrap();
        let (http, mut captured) = make_client_harness(
            AuthenticationMiddleware::from_auth_storage(storage.clone())
                .with_credential_key("conda.example", api_origin.clone()),
        );
        let _ = http.post(format!("{API}/v1/audit")).send().await;
        let request = captured.recv().await.unwrap();
        assert_eq!(header(&request), None);
        assert_eq!(request.url().path(), "/v1/audit");

        // A pinned key with nothing stored stays anonymous.
        let (http, mut captured) = make_client_harness(
            AuthenticationMiddleware::from_auth_storage(storage)
                .with_credential_key("missing", api_origin),
        );
        let _ = http.post(API).send().await;
        assert_eq!(header(&captured.recv().await.unwrap()), None);
    }
}
