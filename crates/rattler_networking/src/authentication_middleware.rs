//! `reqwest` middleware that authenticates requests with data from the
//! `AuthenticationStorage`
use std::{
    collections::HashMap,
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
    credential_sources: HashMap<url::Origin, url::Host<String>>,
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
        let explicit_source = !self.credential_sources.is_empty();
        let selected = if explicit_source {
            let Some(host) = self.credential_sources.get(&url.origin()) else {
                return next.run(req, extensions).await;
            };
            let key = host.to_string();
            self.auth_storage
                .get(&key)
                .map(|auth| (url, auth.map(|auth| (key, auth))))
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
                let auth = match auth_with_key {
                    Some((matched_key, auth)) => {
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

                // Refresh may have picked up a different credential kind.
                if explicit_source
                    && auth.as_ref().is_some_and(|auth| {
                        !matches!(
                            auth,
                            Authentication::OAuth { .. } | Authentication::BearerToken(_)
                        )
                    })
                {
                    tracing::warn!(
                        "Selected credential is not an OAuth or bearer token; not sending it to the configured origin"
                    );
                    return next.run(req, extensions).await;
                }

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
            credential_sources: HashMap::new(),
        }
    }

    /// Create a new authentication middleware with the default authentication
    /// storage
    pub fn from_env_and_defaults() -> Result<Self, AuthenticationStorageError> {
        Ok(Self {
            auth_storage: AuthenticationStorage::from_env_and_defaults()?,
            credential_sources: HashMap::new(),
        })
    }

    /// Reuse `source_host`'s OAuth or bearer credential for `trusted_origins`,
    /// without wildcard fallback. Refresh updates the source credential.
    ///
    /// Calls add mappings; repeated origins replace their source. Empty input
    /// changes nothing. Once configured, unmapped origins and missing or
    /// unsupported credentials remain anonymous.
    ///
    /// Use on a dedicated API client, not stacked with channel authentication.
    /// Disable redirects on the underlying client for credential-bearing requests.
    /// Audience-token refresh is supported on native targets only.
    pub fn with_credentials_from(
        mut self,
        source_host: url::Host<String>,
        trusted_origins: impl IntoIterator<Item = url::Origin>,
    ) -> Self {
        for origin in trusted_origins {
            self.credential_sources.insert(origin, source_host.clone());
        }
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

        for (audience, source_host) in [
            (None, None),
            (Some("https://audit.example"), Some("issuer.example")),
        ] {
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

            let host = source_host.unwrap_or("127.0.0.1").to_owned();
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
            let second_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let second_url = format!("http://{}/repo", second_listener.local_addr().unwrap());
            let second_router = Router::new()
                .route("/repo", post(repo))
                .with_state(state.clone());
            tokio::spawn(async move { axum::serve(second_listener, second_router).await.unwrap() });
            let mut middleware = AuthenticationMiddleware::from_auth_storage(storage.clone());
            if let Some(source_host) = source_host {
                let host = url::Host::parse(source_host).unwrap();
                middleware = middleware.with_credentials_from(
                    host,
                    [
                        Url::parse(&repo_url).unwrap().origin(),
                        Url::parse(&second_url).unwrap().origin(),
                    ],
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

            let responses = join_all((0..8).map(|index| {
                let url = if source_host.is_some() && index % 2 == 1 {
                    &second_url
                } else {
                    &repo_url
                };
                client.post(url).send()
            }))
            .await;
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
    async fn multiple_origins_select_their_source_hosts() {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        for (host, token) in [
            ("first.example", "first"),
            ("second.example", "second"),
            ("unmapped.example", "must-not-fallback"),
            ("*.example", "must-not-use-wildcard"),
        ] {
            storage
                .store(host, &Authentication::BearerToken(token.into()))
                .unwrap();
        }
        let first = url::Host::parse("first.example").unwrap();
        let second = url::Host::parse("second.example").unwrap();
        let origin = |value: &str| Url::parse(value).unwrap().origin();
        let middleware = AuthenticationMiddleware::from_auth_storage(storage)
            .with_credentials_from(
                first,
                [origin("https://one.example"), origin("https://two.example")],
            )
            .with_credentials_from(second.clone(), [origin("https://three.example")])
            .with_credentials_from(second, [origin("https://one.example")])
            .with_credentials_from(
                url::Host::parse("missing.example").unwrap(),
                [origin("https://missing-api.example")],
            );
        let (http, mut captured) = make_client_harness(middleware);
        for (url, expected) in [
            ("https://one.example/path", Some("Bearer second")),
            ("https://two.example/path", Some("Bearer first")),
            ("https://three.example/path", Some("Bearer second")),
            ("https://missing-api.example", None),
            ("https://unmapped.example", None),
            ("https://two.example:444/path", None),
            ("http://two.example/path", None),
        ] {
            let _ = http.get(url).send().await;
            let request = captured.recv().await.unwrap();
            assert_eq!(
                request
                    .headers()
                    .get("authorization")
                    .map(|v| v.to_str().unwrap()),
                expected,
                "{url}"
            );
        }
    }

    #[tokio::test]
    async fn empty_origins_preserve_existing_credential_selection() {
        let api = Url::parse("https://api.example").unwrap();
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        for (key, token) in [("api.example", "destination"), ("issuer.example", "source")] {
            storage
                .store(key, &Authentication::BearerToken(token.into()))
                .unwrap();
        }
        let base = AuthenticationMiddleware::from_auth_storage(storage);
        let host = url::Host::parse("issuer.example").unwrap();
        for (middleware, expected) in [
            (base.clone(), "Bearer destination"),
            (
                base.with_credentials_from(host.clone(), [api.origin()]),
                "Bearer source",
            ),
        ] {
            let (http, mut captured) =
                make_client_harness(middleware.with_credentials_from(host.clone(), []));
            let _ = http.get(api.clone()).send().await;
            let request = captured.recv().await.unwrap();
            assert_eq!(request.headers().get("authorization").unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn source_hosts_use_canonical_domain_and_ip_storage_keys() {
        let api = Url::parse("https://api.example").unwrap();
        for (source, key) in [
            ("ISSUER.EXAMPLE", "issuer.example"),
            ("127.0.0.1", "127.0.0.1"),
            ("[::1]", "[::1]"),
        ] {
            let mut storage = AuthenticationStorage::empty();
            storage.add_backend(Arc::new(MemoryStorage::new()));
            storage
                .store(key, &Authentication::BearerToken("fixture".into()))
                .unwrap();
            let middleware = AuthenticationMiddleware::from_auth_storage(storage)
                .with_credentials_from(url::Host::parse(source).unwrap(), [api.origin()]);
            let (http, mut captured) = make_client_harness(middleware);
            let _ = http.get(api.clone()).send().await;
            let request = captured.recv().await.unwrap();
            assert_eq!(
                request.headers().get("authorization").unwrap(),
                "Bearer fixture"
            );
        }
    }

    #[tokio::test]
    async fn replacement_during_refresh_cannot_change_reused_credential_kind() {
        let source = "issuer.example";
        let api = Url::parse("https://api.example/v1/audit").unwrap();
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        storage
            .store(
                source,
                &Authentication::OAuth {
                    audience: Some(api.origin().ascii_serialization()),
                    access_token: "expired".into(),
                    refresh_token: Some("refresh".into()),
                    expires_at: Some(0),
                    token_endpoint: "http://127.0.0.1:1/token".into(),
                    revocation_endpoint: None,
                    client_id: "client".into(),
                },
            )
            .unwrap();
        let middleware = AuthenticationMiddleware::from_auth_storage(storage.clone())
            .with_credentials_from(url::Host::parse(source).unwrap(), [api.origin()]);
        let (http, mut captured) = make_client_harness(middleware);
        let lock = storage.oauth_refresh_lock(source);
        let guard = lock.lock().await;
        let request = http.post(api.clone()).send();
        tokio::pin!(request);
        assert!(futures::poll!(&mut request).is_pending());
        storage
            .store(source, &Authentication::CondaToken("must-not-leak".into()))
            .unwrap();
        drop(guard);
        let _ = request.await;
        let request = captured.recv().await.unwrap();
        assert_eq!(request.url(), &api);
        assert!(!request.headers().contains_key("authorization"));
    }

    #[tokio::test]
    async fn reused_host_credential_is_sent_only_to_trusted_origin() {
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

        // Default host and wildcard lookup.
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

        // Explicit origin mapping.
        let mapped = AuthenticationMiddleware::from_auth_storage(storage.clone())
            .with_credentials_from(
                url::Host::parse("issuer.example").unwrap(),
                [api_origin.clone()],
            );
        for (url, expected) in [
            (format!("{API}/v1/audit"), Some("Bearer fixture.host.token")),
            ("https://issuer.example/channel".to_string(), None),
            ("https://api.basilisk.example:444/".to_string(), None),
            ("http://api.basilisk.example/".to_string(), None),
        ] {
            let (http, mut captured) = make_client_harness(mapped.clone());
            let _ = http.post(&url).send().await;
            assert_eq!(
                header(&captured.recv().await.unwrap()).as_deref(),
                expected,
                "{url}"
            );
        }

        storage
            .store(
                "conda.example",
                &Authentication::CondaToken("fixture-conda".into()),
            )
            .unwrap();
        let (http, mut captured) = make_client_harness(
            AuthenticationMiddleware::from_auth_storage(storage.clone()).with_credentials_from(
                url::Host::parse("conda.example").unwrap(),
                [api_origin.clone()],
            ),
        );
        let _ = http.post(format!("{API}/v1/audit")).send().await;
        let request = captured.recv().await.unwrap();
        assert_eq!(header(&request), None);
        assert_eq!(request.url().path(), "/v1/audit");

        let (http, mut captured) = make_client_harness(
            AuthenticationMiddleware::from_auth_storage(storage)
                .with_credentials_from(url::Host::parse("missing.example").unwrap(), [api_origin]),
        );
        let _ = http.post(API).send().await;
        assert_eq!(header(&captured.recv().await.unwrap()), None);
    }
}
