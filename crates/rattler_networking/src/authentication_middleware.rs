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
        match self.auth_storage.get_by_url_with_host(url) {
            Err(_) => {
                // Forward error to caller (invalid URL)
                next.run(req, extensions).await
            }
            Ok((url, auth_with_key)) => {
                // If this is an OAuth token, attempt refresh if expired
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
        Self { auth_storage }
    }

    /// Create a new authentication middleware with the default authentication
    /// storage
    pub fn from_env_and_defaults() -> Result<Self, AuthenticationStorageError> {
        Ok(Self {
            auth_storage: AuthenticationStorage::from_env_and_defaults()?,
        })
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
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[cfg(feature = "keyring")]
    use anyhow::anyhow;
    use axum::{
        Json, Router,
        extract::State,
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
            Err(reqwest_middleware::Error::Middleware(anyhow!(
                "captured request, aborting"
            )))
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
    fn test_store_fallback() -> anyhow::Result<()> {
        let tdir = tempdir()?;
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::from(FileStorage::from_path(
            tdir.path().to_path_buf().join("auth.json"),
        )?));

        let host = "test.example.com";
        let authentication = Authentication::CondaToken("testtoken".to_string());
        storage.store(host, &authentication)?;
        storage.delete(host)?;
        Ok(())
    }

    #[cfg(feature = "keyring")]
    #[tokio::test]
    async fn test_conda_token_storage() -> anyhow::Result<()> {
        let tdir = tempdir()?;
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::from(FileStorage::from_path(
            tdir.path().to_path_buf().join("auth.json"),
        )?));

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
        storage.store(host, &authentication)?;

        let retrieved = storage.get(host);
        assert!(retrieved.is_ok());
        let retrieved = retrieved.unwrap();
        assert!(retrieved.is_some());
        let auth = retrieved.unwrap();
        assert!(auth == authentication);

        let (client, mut captured_rx) = make_client_harness(&storage);

        let request = client.get("https://conda.example.com/conda-forge/noarch/testpkg.tar.bz2");
        let request = request.build()?;

        // we expect middleware error. if auth middleware fails, tests below will detect
        // it
        let _ = client.execute(request).await;

        let captured_request = captured_rx.recv().await.unwrap();
        assert!(captured_request.url().path().starts_with("/t/testtoken"));

        storage.delete(host)?;
        Ok(())
    }

    #[cfg(feature = "keyring")]
    #[tokio::test]
    async fn test_bearer_storage() -> anyhow::Result<()> {
        let tdir = tempdir()?;
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::from(FileStorage::from_path(
            tdir.path().to_path_buf().join("auth.json"),
        )?));
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

        storage.store(host, &authentication)?;

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

        storage.delete(host)?;
        Ok(())
    }

    #[cfg(feature = "keyring")]
    #[tokio::test]
    async fn test_basic_auth_storage() -> anyhow::Result<()> {
        let tdir = tempdir()?;
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::from(FileStorage::from_path(
            tdir.path().to_path_buf().join("auth.json"),
        )?));
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
        storage.store(host, &authentication)?;

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

        storage.delete(host)?;
        Ok(())
    }

    #[test]
    fn test_host_wildcard_expansion() -> anyhow::Result<()> {
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
            let tdir = tempdir()?;
            let mut storage = AuthenticationStorage::empty();
            storage.add_backend(Arc::from(FileStorage::from_path(
                tdir.path().to_path_buf().join("auth.json"),
            )?));

            let authentication = Authentication::BearerToken("testtoken".to_string());

            storage.store(host, &authentication)?;

            let retrieved =
                storage.get_by_url("https://repo.prefix.dev/conda-forge/noarch/repodata.json")?;

            if should_succeed {
                assert_eq!(retrieved.1, Some(authentication));
            } else {
                assert_eq!(retrieved.1, None);
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn concurrent_oauth_refresh_is_coalesced_by_authentication_middleware()
    -> anyhow::Result<()> {
        #[derive(Clone)]
        struct TestState {
            refresh_count: Arc<AtomicUsize>,
            seen_authorization: Arc<Mutex<Vec<Option<String>>>>,
        }

        async fn token(State(state): State<TestState>) -> (StatusCode, Json<serde_json::Value>) {
            state.refresh_count.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(json!({
                    "access_token": "fresh-access-token",
                    "refresh_token": "rotated-refresh-token",
                    "expires_in": 3600,
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

        let state = TestState {
            refresh_count: Arc::new(AtomicUsize::new(0)),
            seen_authorization: Arc::new(Mutex::new(Vec::new())),
        };
        let router = Router::new()
            .route("/token", post(token))
            .route("/repo", post(repo))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let host = "127.0.0.1";
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        storage.store(
            host,
            &Authentication::OAuth {
                access_token: "expired-access-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
                expires_at: Some(0),
                token_endpoint: format!("http://{addr}/token"),
                revocation_endpoint: None,
                client_id: "client-id".to_string(),
            },
        )?;

        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::default())
            .with(AuthenticationMiddleware::from_auth_storage(storage))
            .build();
        let repo_url = format!("http://{addr}/repo");

        let responses = join_all((0..8).map(|_| client.post(&repo_url).send())).await;
        for response in responses {
            assert_eq!(response?.status(), StatusCode::OK);
        }

        assert_eq!(state.refresh_count.load(Ordering::SeqCst), 1);
        let seen_authorization = state.seen_authorization.lock().unwrap();
        assert_eq!(seen_authorization.len(), 8);
        assert!(
            seen_authorization
                .iter()
                .all(|auth| { auth.as_deref() == Some("Bearer fresh-access-token") })
        );

        Ok(())
    }

    #[tokio::test]
    async fn expired_oauth_with_failed_refresh_sends_no_authorization_header() -> anyhow::Result<()>
    {
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let host = "127.0.0.1";
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        storage.store(
            host,
            &Authentication::OAuth {
                access_token: "expired-access-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
                expires_at: Some(0),
                token_endpoint: format!("http://{addr}/token"),
                revocation_endpoint: None,
                client_id: "client-id".to_string(),
            },
        )?;

        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::default())
            .with(AuthenticationMiddleware::from_auth_storage(storage))
            .build();

        let response = client.post(format!("http://{addr}/repo")).send().await?;
        assert_eq!(response.status(), StatusCode::OK);

        // Refresh failed and the access token is expired, so no expired bearer
        // token should leak to the backend.
        let seen_authorization = state.seen_authorization.lock().unwrap();
        assert_eq!(seen_authorization.as_slice(), &[None]);

        Ok(())
    }

    #[test]
    fn test_rattler_auth_file_env_var_handling() -> anyhow::Result<()> {
        let tdir = tempdir()?;

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
        storage.store(host, &authentication)?;

        let file = tdir.path().join("auth.json");
        assert_eq!(
            std::fs::read_to_string(file)?,
            "{\"test.example.com\":{\"CondaToken\":\"testtoken\"}}"
        );

        Ok(())
    }
}
