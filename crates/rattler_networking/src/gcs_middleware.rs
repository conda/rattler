//! Middleware to handle `gcs://` URLs to pull artifacts from an GCS
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use google_cloud_auth::credentials::{
    Builder as AccessTokenCredentialBuilder, CacheableResource, Credentials,
};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use tokio::sync::Mutex;
use url::Url;

/// Google `OAuth2` tokens are valid for 3600 seconds. We refresh 5 minutes
/// before expiry so that any in-flight requests that obtained the token just
/// before the deadline still have plenty of validity left.
const TOKEN_VALID_FOR: Duration = Duration::from_secs(3600 - 300);

/// A cached GCS bearer-token and the instant at which it should be refreshed.
struct CachedToken {
    headers: http::HeaderMap,
    valid_until: Instant,
}

/// Shared, ref-counted state owned by every clone of a [`GCSMiddleware`].
struct GCSInner {
    /// Credential source built once and reused across all requests.
    credential: Mutex<Option<Credentials>>,
    /// Most-recently-fetched auth headers; reused until near expiry.
    token: Mutex<Option<CachedToken>>,
}

/// GCS middleware to authenticate requests.
///
/// A single [`GCSMiddleware`] instance (or any clone of one) shares one `OAuth2`
/// credential and one token cache, so only one token-endpoint round-trip is
/// made per token lifetime (~55 minutes) rather than one per package download.
#[derive(Clone)]
pub struct GCSMiddleware {
    inner: Arc<GCSInner>,
}

impl Default for GCSMiddleware {
    fn default() -> Self {
        Self {
            inner: Arc::new(GCSInner {
                credential: Mutex::new(None),
                token: Mutex::new(None),
            }),
        }
    }
}

#[async_trait]
impl Middleware for GCSMiddleware {
    /// Create a new authentication middleware for GCS
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        if req.url().scheme() == "gcs" {
            let mut url = req.url().clone();
            let bucket_name = url.host_str().expect("Host should be present in GCS URL");
            let new_url = format!(
                "https://storage.googleapis.com/{}{}",
                bucket_name,
                url.path()
            );
            url = Url::parse(&new_url).expect("Failed to parse URL");
            *req.url_mut() = url;
            req = self.authenticate(req).await?;
        }
        next.run(req, extensions).await
    }
}

impl GCSMiddleware {
    /// Add GCS authentication headers to `req`, drawing from the token cache
    /// when available and fetching a new token only when necessary.
    async fn authenticate(&self, mut req: Request) -> MiddlewareResult<Request> {
        let headers = self.get_or_refresh_token().await?;
        req.headers_mut().extend(headers);
        Ok(req)
    }

    /// Return cached auth headers if the token is still valid, otherwise
    /// obtain a fresh token from Google and update the cache.
    async fn get_or_refresh_token(&self) -> MiddlewareResult<http::HeaderMap> {
        // Fast path: reuse the cached token if it has not expired yet.
        {
            let guard = self.inner.token.lock().await;
            if let Some(t) = guard.as_ref() {
                if t.valid_until > Instant::now() {
                    return Ok(t.headers.clone());
                }
            }
        }

        // Slow path: lazily build the credential once, then fetch a new token.
        let cred = {
            let mut guard = self.inner.credential.lock().await;
            if guard.is_none() {
                let scopes = ["https://www.googleapis.com/auth/devstorage.read_only"];
                let c = AccessTokenCredentialBuilder::default()
                    .with_scopes(scopes)
                    .build()
                    .map_err(|e| reqwest_middleware::Error::Middleware(anyhow::Error::new(e)))?;
                *guard = Some(c);
            }
            // Credentials is Arc-backed; clone is a cheap refcount bump.
            guard.as_ref().unwrap().clone()
        };

        let headers = match cred.headers(http::Extensions::new()).await {
            Ok(CacheableResource::New { data, .. }) => data,
            Ok(CacheableResource::NotModified) => {
                // We never pass a prior entity-tag via extensions, so the
                // library cannot return NotModified. Treat it as a library
                // bug and fall back to the fresh-fetch path.
                unreachable!(
                    "no entity tag was provided in extensions, \
                     so NotModified cannot be returned"
                )
            }
            Err(e) => {
                return Err(reqwest_middleware::Error::Middleware(anyhow::Error::new(e)));
            }
        };

        // Store the token for re-use across subsequent requests.
        {
            let mut guard = self.inner.token.lock().await;
            *guard = Some(CachedToken {
                headers: headers.clone(),
                valid_until: Instant::now() + TOKEN_VALID_FOR,
            });
        }

        Ok(headers)
    }
}

#[cfg(test)]
mod tests {
    use reqwest::Client;
    use tempfile;

    use super::*;

    #[tokio::test]
    async fn test_gcs_middleware() {
        let credentials = match std::env::var("GOOGLE_CLOUD_TEST_KEY_JSON") {
            Ok(credentials) if !credentials.is_empty() => credentials,
            Err(_) | Ok(_) => {
                eprintln!("Skipping test as GOOGLE_CLOUD_TEST_KEY_JSON is not set");
                return;
            }
        };
        println!("Running GCS Test");

        // We have to set GOOGLE_APPLICATION_CREDENTIALS to the path of the JSON key
        // file
        let key_file = tempfile::NamedTempFile::with_suffix(".json").unwrap();
        std::fs::write(&key_file, credentials).unwrap();

        let prev_value = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok();
        std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", key_file.path());

        let client = reqwest_middleware::ClientBuilder::new(Client::new())
            .with(GCSMiddleware::default())
            .build();

        let url = "gcs://test-channel/noarch/repodata.json";
        let response = client.get(url).send().await.unwrap();
        assert!(response.status().is_success());

        let url = "gcs://test-channel-nonexist/noarch/repodata.json";
        let response = client.get(url).send().await.unwrap();
        assert!(response.status().is_client_error());

        if let Some(value) = prev_value {
            std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", value);
        } else {
            std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        }
    }
}
