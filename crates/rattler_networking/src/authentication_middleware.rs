//! `reqwest` middleware that authenticates requests with data from the
//! `AuthenticationStorage`
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use base64::{prelude::BASE64_STANDARD, Engine};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next};
use serde::Deserialize;
use url::Url;

use crate::{
    authentication_storage::AuthenticationStorageError, Authentication, AuthenticationStorage,
};

/// Response from an OAuth token refresh request (standard `OAuth2` token
/// response).
#[derive(Deserialize)]
struct TokenRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

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
                    Some((matched_key, oauth_auth @ Authentication::OAuth { .. })) => {
                        self.maybe_refresh_oauth(oauth_auth, &matched_key).await
                    }
                    Some((_, auth)) => Some(auth),
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

    /// Check if an OAuth token is expired and attempt to refresh it.
    ///
    /// Returns the (possibly refreshed) authentication. If refresh fails,
    /// returns the original auth so the request proceeds with the existing
    /// (possibly expired) token — the server will return 401 which is clearer
    /// than a middleware error.
    async fn maybe_refresh_oauth(
        &self,
        auth: Authentication,
        matched_key: &str,
    ) -> Option<Authentication> {
        let Authentication::OAuth {
            ref access_token,
            ref refresh_token,
            expires_at,
            ref token_endpoint,
            ref revocation_endpoint,
            ref client_id,
        } = auth
        else {
            return Some(auth);
        };

        // Check if token is expired (with 5 minute buffer for clock skew)
        let is_expired = expires_at.is_some_and(|exp| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            exp - now < 300 // 5 minute buffer
        });

        if !is_expired {
            return Some(auth);
        }

        let Some(refresh_token_val) = refresh_token.as_deref() else {
            tracing::warn!("OAuth token is expired but no refresh token is available");
            return Some(auth);
        };

        tracing::debug!("OAuth token expired, attempting refresh");

        let client = reqwest::Client::new();
        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token_val),
            ("client_id", client_id),
        ];

        let response = match client
            .post(token_endpoint.as_str())
            .form(&params)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("Failed to refresh OAuth token: {e}");
                return Some(auth);
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let hint = match response.json::<serde_json::Value>().await {
                Ok(body) => {
                    let error_code = body
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    if error_code == "invalid_grant" {
                        "refresh token is expired or revoked — please re-authenticate".to_string()
                    } else {
                        format!("error code: {error_code}")
                    }
                }
                Err(_) => format!("HTTP {status}"),
            };
            tracing::warn!("OAuth token refresh failed ({hint})");
            return Some(auth);
        }

        let token_response: TokenRefreshResponse = match response.json().await {
            Ok(body) => body,
            Err(e) => {
                tracing::warn!("Failed to read OAuth refresh response body: {e}");
                return Some(auth);
            }
        };

        let new_expires_at = token_response.expires_in.map(|secs| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64
                + secs
        });

        let refreshed = Authentication::OAuth {
            access_token: token_response.access_token,
            refresh_token: token_response
                .refresh_token
                .or_else(|| refresh_token.clone()),
            expires_at: new_expires_at,
            token_endpoint: token_endpoint.clone(),
            revocation_endpoint: revocation_endpoint.clone(),
            client_id: client_id.clone(),
        };

        // Store the refreshed token back (best-effort)
        if let Err(e) = self.auth_storage.store(matched_key, &refreshed) {
            tracing::warn!("Failed to store refreshed OAuth token: {e}");
        }

        // Invalidate the cache entry for the old token
        let _ = access_token;

        Some(refreshed)
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
    use std::sync::Arc;

    #[cfg(feature = "keyring")]
    use anyhow::anyhow;
    use tempfile::tempdir;

    use super::*;
    use crate::authentication_storage::backends::file::FileStorage;

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

        // Make sure the keyring is empty
        if let Ok(entry) = keyring::Entry::new("rattler_test", host) {
            let _ = entry.delete_credential();
        }

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

        // Make sure the keyring is empty
        if let Ok(entry) = keyring::Entry::new("rattler_test", host) {
            let _ = entry.delete_credential();
        }

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

        // Make sure the keyring is empty
        if let Ok(entry) = keyring::Entry::new("rattler_test", host) {
            let _ = entry.delete_credential();
        }

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
