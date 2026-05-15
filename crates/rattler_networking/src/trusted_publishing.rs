// This code has been adapted from uv under https://github.com/astral-sh/uv/blob/c5caf92edf539a9ebf24d375871178f8f8a0ab93/crates/uv-publish/src/trusted_publishing.rs
// The original code is dual-licensed under Apache-2.0 and MIT

//! Trusted publishing (via OIDC) with GitHub Actions, GitLab CI, and Google
//! Cloud.
//!
//! The flow:
//! 1. Detect which CI provider we are running on.
//! 2. Ask the CI's OIDC provider for an ID token with the configured
//!    `audience` claim.
//! 3. Exchange that ID token at the server's mint endpoint for a short-lived
//!    bearer token usable against the server (read or write, depending on
//!    server policy).

use std::{
    env,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use reqwest::StatusCode;
use reqwest_middleware::{ClientWithMiddleware, Middleware, Next};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

/// Refresh minted JWT tokens before they expire to avoid sending a token that
/// becomes invalid while a request is in flight.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// Environment-variable names sniffed by [`detect_ci_provider`] to gate which
/// CI providers we attempt trusted publishing on. Token retrieval itself is
/// delegated to `ambient-id`, which owns the OIDC-specific env vars.
mod consts {
    pub const GITHUB_ACTIONS: &str = "GITHUB_ACTIONS";
    pub const GITLAB_CI: &str = "GITLAB_CI";
    pub const CLOUD_BUILD_ID: &str = "CLOUD_BUILD_ID";
    pub const K_SERVICE: &str = "K_SERVICE";
}

/// Default audience for the OIDC ID token. Matches prefix.dev's expectation.
pub const DEFAULT_AUDIENCE: &str = "prefix.dev";

/// Default path on the server where the OIDC ID token is exchanged for a
/// bearer token.
pub const DEFAULT_MINT_PATH: &str = "/api/oidc/mint_token";

/// Knobs for the trusted-publishing flow. Defaults target prefix.dev; override
/// any field to point at a different server.
///
/// On GitLab CI, the OIDC ID token must be populated by the runner under an
/// env var whose name is derived from [`audience`](Self::audience) by
/// `ambient-id` (uppercasing the audience and replacing non-alphanumeric
/// characters with `_`, then suffixing `_ID_TOKEN`). For the default audience
/// `prefix.dev`, that resolves to `PREFIX_DEV_ID_TOKEN` — set this via the
/// `id_tokens` block in `.gitlab-ci.yml`.
#[derive(Debug, Clone)]
pub struct TrustedPublishingOptions {
    /// The `aud` claim requested in the OIDC ID token. The server validates
    /// this against the trusted-publisher configuration before minting a
    /// token.
    pub audience: String,
    /// Path on the server (joined onto `server_url`) where the ID token is
    /// exchanged for a bearer token.
    pub mint_path: String,
}

impl Default for TrustedPublishingOptions {
    fn default() -> Self {
        Self {
            audience: DEFAULT_AUDIENCE.to_string(),
            mint_path: DEFAULT_MINT_PATH.to_string(),
        }
    }
}

fn github_action_runner() -> bool {
    env::var(consts::GITHUB_ACTIONS) == Ok("true".to_string())
}

fn gitlab_ci_runner() -> bool {
    env::var(consts::GITLAB_CI) == Ok("true".to_string())
}

fn google_cloud_runner() -> bool {
    env::var(consts::CLOUD_BUILD_ID).is_ok() || env::var(consts::K_SERVICE).is_ok()
}

/// Returns `true` if we're running on one of the CI providers supported for
/// trusted publishing (GitHub Actions, GitLab CI, or Google Cloud).
///
/// This gates token retrieval: `ambient-id` also supports `CircleCI` and
/// Buildkite, but we don't expose those here to keep the set of providers
/// predictable for the server side.
fn on_supported_ci() -> bool {
    github_action_runner() || gitlab_ci_runner() || google_cloud_runner()
}

/// Outcome of an optional trusted-publishing attempt.
pub enum TrustedPublishResult {
    /// We didn't check for trusted publishing (no CI provider detected).
    Skipped,
    /// We checked for trusted publishing and got a token.
    Configured(TrustedPublishingToken),
    /// We checked for optional trusted publishing, but it didn't succeed.
    Ignored(TrustedPublishingError),
}

/// Errors that can occur during the trusted-publishing flow.
#[derive(Debug, Error)]
pub enum TrustedPublishingError {
    /// Failed to parse a URL.
    #[error(transparent)]
    Url(#[from] url::ParseError),
    /// HTTP request failed at the reqwest layer.
    #[error("Failed to fetch: `{0}`")]
    Reqwest(Url, #[source] reqwest::Error),
    /// HTTP request failed at the reqwest-middleware layer.
    #[error("Failed to fetch: `{0}`")]
    ReqwestMiddleware(Url, #[source] reqwest_middleware::Error),
    /// The mint endpoint returned an error.
    #[error(
        "Server returned error code {0} from the mint endpoint, is trusted publishing correctly configured?\nResponse: {1}"
    )]
    MintToken(StatusCode, String),
    /// Retrieving the OIDC ID token from the CI provider failed.
    #[error("Failed to retrieve an OIDC ID token from the CI provider")]
    OidcToken(#[from] ambient_id::Error),
    /// We detected a supported CI environment but `ambient-id` returned no
    /// token. This indicates a mismatch between our env-var gate and
    /// `ambient-id`'s internal detection — most commonly, missing
    /// provider-specific permissions (e.g., `id-token: write` on GitHub
    /// Actions).
    #[error(
        "Detected a supported CI environment but no OIDC ID token was issued — check that the required permissions are configured (e.g. `id-token: write` on GitHub Actions)"
    )]
    NoOidcToken,
}

/// A short-lived bearer token minted by the server. The inner string is
/// `Deserialize`-friendly (the mint endpoint returns the raw token as a JSON
/// string body) and `Clone` so the same token can be shared across middleware
/// and stored in auth state.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct TrustedPublishingToken(String);

impl TrustedPublishingToken {
    /// Wrap an already-minted token (mostly for tests).
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// The raw bearer token. Treat as sensitive; don't log it.
    pub fn secret(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for TrustedPublishingToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TrustedPublishingToken")
            .field(&"<redacted>")
            .finish()
    }
}

/// The body sent to the server's mint endpoint.
#[derive(Serialize)]
struct MintTokenRequest {
    token: String,
}

#[derive(Deserialize)]
struct JwtClaims {
    exp: Option<u64>,
}

fn jwt_expiration(token: &str) -> Option<SystemTime> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _signature = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    let claims: JwtClaims = serde_json::from_slice(&payload).ok()?;
    claims
        .exp
        .and_then(|exp| UNIX_EPOCH.checked_add(Duration::from_secs(exp)))
}

/// If applicable, attempt to obtain a bearer token via trusted publishing.
///
/// Returns [`TrustedPublishResult::Skipped`] when no CI provider is detected
/// (the common case outside CI). Errors during the flow are wrapped in
/// [`TrustedPublishResult::Ignored`] so callers can fall back to other auth
/// sources without unwinding.
pub async fn check_trusted_publishing(
    client: &ClientWithMiddleware,
    server_url: &Url,
    options: &TrustedPublishingOptions,
) -> TrustedPublishResult {
    if !on_supported_ci() {
        return TrustedPublishResult::Skipped;
    }

    tracing::debug!(
        "Running on a supported CI environment without explicit credentials, checking for trusted publishing"
    );

    match get_token(client, server_url, options).await {
        Ok(token) => TrustedPublishResult::Configured(token),
        Err(err) => {
            tracing::debug!("Could not obtain trusted publishing credentials, skipping: {err}");
            TrustedPublishResult::Ignored(err)
        }
    }
}

/// Returns the short-lived token to use against `server_url`.
///
/// Delegates OIDC ID-token retrieval to `ambient-id`; this function owns the
/// mint exchange with `server_url`.
pub async fn get_token(
    client: &ClientWithMiddleware,
    server_url: &Url,
    options: &TrustedPublishingOptions,
) -> Result<TrustedPublishingToken, TrustedPublishingError> {
    let detector = ambient_id::Detector::new_with_client(client.clone());
    let oidc_token = match detector.detect(&options.audience).await? {
        Some(token) => token,
        None => return Err(TrustedPublishingError::NoOidcToken),
    };

    let publish_token = get_publish_token(oidc_token.reveal(), server_url, client, options).await?;

    tracing::info!("Received OIDC token from CI provider, using trusted publishing");

    // Mask the token in GitHub Actions logs so the bearer doesn't leak into
    // CI output. The `::add-mask::` workflow command is a no-op outside GHA.
    if github_action_runner() {
        println!("::add-mask::{}", publish_token.secret());
    }

    Ok(publish_token)
}

async fn get_publish_token(
    oidc_token: &str,
    server_url: &Url,
    client: &ClientWithMiddleware,
    options: &TrustedPublishingOptions,
) -> Result<TrustedPublishingToken, TrustedPublishingError> {
    let mint_token_url = server_url.join(&options.mint_path)?;
    tracing::info!("Querying the trusted publishing token from {mint_token_url}");
    let mint_token_payload = MintTokenRequest {
        token: oidc_token.to_string(),
    };

    let response = client
        .post(mint_token_url.clone())
        .json(&mint_token_payload)
        .send()
        .await
        .map_err(|err| TrustedPublishingError::ReqwestMiddleware(mint_token_url.clone(), err))?;

    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|err| TrustedPublishingError::Reqwest(mint_token_url.clone(), err))?;

    if status.is_success() {
        Ok(TrustedPublishingToken(
            String::from_utf8_lossy(&body).to_string(),
        ))
    } else {
        Err(TrustedPublishingError::MintToken(
            status,
            String::from_utf8_lossy(&body).to_string(),
        ))
    }
}

/// `reqwest` middleware that injects a [`TrustedPublishingToken`] as a
/// `Bearer` `Authorization` header for requests targeting a specific channel.
///
/// Layered alongside [`crate::AuthenticationMiddleware`]: it only sets the
/// header when no `Authorization` is already present and only when the
/// request URL's host and path prefix match the configured channel. This
/// keeps the minted token scoped to the channel it was issued for, instead
/// of leaking it to unrelated channels (which may share a host, e.g.
/// `https://prefix.dev/my-channel/` vs `https://prefix.dev/other-channel/`).
#[derive(Clone, Debug)]
pub struct TrustedPublishingMiddleware {
    channel_url: Url,
    state: TrustedPublishingState,
}

#[derive(Clone, Debug)]
enum TrustedPublishingState {
    /// Caller supplied an already-minted token.
    Token(TrustedPublishingToken),
    /// Token will be minted on the first matching request.
    Lazy {
        server_url: Url,
        options: TrustedPublishingOptions,
        client: ClientWithMiddleware,
        cache: Arc<Mutex<TrustedPublishingCache>>,
    },
}

#[derive(Debug, Default)]
enum TrustedPublishingCache {
    #[default]
    Empty,
    Disabled,
    Token(CachedTrustedPublishingToken),
}

#[derive(Clone, Debug)]
struct CachedTrustedPublishingToken {
    token: TrustedPublishingToken,
    expires_at: Option<SystemTime>,
}

impl CachedTrustedPublishingToken {
    fn new(token: TrustedPublishingToken) -> Self {
        let expires_at = jwt_expiration(token.secret());
        Self { token, expires_at }
    }

    fn is_fresh(&self, now: SystemTime) -> bool {
        self.expires_at
            .is_none_or(|expires_at| now + TOKEN_REFRESH_MARGIN < expires_at)
    }
}

impl TrustedPublishingMiddleware {
    /// Create a middleware that will mint a token lazily on the first
    /// matching request using `options`. `client` is used only for the OIDC
    /// mint exchange; it must not itself layer in `TrustedPublishingMiddleware`
    /// or the mint call will recurse.
    pub fn new(
        server_url: Url,
        options: TrustedPublishingOptions,
        client: ClientWithMiddleware,
    ) -> Self {
        let channel_url = normalize_channel_url(&server_url);
        Self {
            channel_url,
            state: TrustedPublishingState::Lazy {
                server_url,
                options,
                client,
                cache: Arc::new(Mutex::new(TrustedPublishingCache::Empty)),
            },
        }
    }

    /// Create a middleware that injects an already-minted `token` on
    /// requests whose URL host and path prefix match `server_url`.
    pub fn with_token(server_url: &Url, token: TrustedPublishingToken) -> Self {
        Self {
            channel_url: normalize_channel_url(server_url),
            state: TrustedPublishingState::Token(token),
        }
    }

    /// Resolve the token to inject, performing (and caching) the OIDC
    /// exchange on demand for the `Lazy` variant.
    async fn token(&self) -> Option<TrustedPublishingToken> {
        match &self.state {
            TrustedPublishingState::Token(token) => Some(token.clone()),
            TrustedPublishingState::Lazy {
                server_url,
                options,
                client,
                cache,
            } => {
                {
                    let cache = cache.lock().expect("trusted publishing cache poisoned");
                    match &*cache {
                        TrustedPublishingCache::Token(token)
                            if token.is_fresh(SystemTime::now()) =>
                        {
                            return Some(token.token.clone());
                        }
                        TrustedPublishingCache::Disabled => return None,
                        TrustedPublishingCache::Empty | TrustedPublishingCache::Token(_) => {}
                    }
                }

                let token = match check_trusted_publishing(client, server_url, options).await {
                    TrustedPublishResult::Configured(token) => Some(token),
                    TrustedPublishResult::Skipped => {
                        tracing::debug!(
                            "TrustedPublishingMiddleware: no CI provider detected, skipping OIDC token exchange"
                        );
                        None
                    }
                    TrustedPublishResult::Ignored(err) => {
                        tracing::warn!(
                            "TrustedPublishingMiddleware: trusted publishing failed: {err}"
                        );
                        None
                    }
                };

                let mut cache = cache.lock().expect("trusted publishing cache poisoned");
                if let Some(token) = token {
                    let token = CachedTrustedPublishingToken::new(token);
                    let result = token.token.clone();
                    *cache = TrustedPublishingCache::Token(token);
                    Some(result)
                } else {
                    *cache = TrustedPublishingCache::Disabled;
                    None
                }
            }
        }
    }
}

/// Normalize a channel URL so its path always ends with `/`. This avoids a
/// prefix-collision bug where `/my-channel` would also match `/my-channel-evil`.
fn normalize_channel_url(url: &Url) -> Url {
    let mut url = url.clone();
    if !url.path().ends_with('/') {
        let new_path = format!("{}/", url.path());
        url.set_path(&new_path);
    }
    url
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Middleware for TrustedPublishingMiddleware {
    async fn handle(
        &self,
        mut req: reqwest::Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        if req.headers().get(reqwest::header::AUTHORIZATION).is_none()
            && req.url().host_str() == self.channel_url.host_str()
            && req.url().path().starts_with(self.channel_url.path())
            && let Some(token) = self.token().await
        {
            let bearer_auth = format!("Bearer {}", token.secret());
            let mut header_value = reqwest::header::HeaderValue::from_str(&bearer_auth)
                .map_err(reqwest_middleware::Error::middleware)?;
            header_value.set_sensitive(true);
            req.headers_mut()
                .insert(reqwest::header::AUTHORIZATION, header_value);
        }
        next.run(req, extensions).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_defaults_match_prefix_dev() {
        let opts = TrustedPublishingOptions::default();
        assert_eq!(opts.audience, "prefix.dev");
        assert_eq!(opts.mint_path, "/api/oidc/mint_token");
    }

    #[test]
    fn token_debug_is_redacted() {
        let token = TrustedPublishingToken::new("supersecret".to_string());
        let formatted = format!("{token:?}");
        assert!(!formatted.contains("supersecret"));
        assert!(formatted.contains("redacted"));
    }

    fn unsigned_jwt_with_exp(exp: u64) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("{header}.{payload}.")
    }

    #[test]
    fn jwt_expiration_reads_exp_claim() {
        let token = unsigned_jwt_with_exp(1_700_000_000);
        assert_eq!(
            jwt_expiration(&token),
            UNIX_EPOCH.checked_add(Duration::from_secs(1_700_000_000))
        );
    }

    #[test]
    fn cached_jwt_is_stale_inside_refresh_margin() {
        let token = TrustedPublishingToken::new(unsigned_jwt_with_exp(1_700_000_000));
        let cached = CachedTrustedPublishingToken::new(token);
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000 - 30);
        assert!(!cached.is_fresh(now));
    }

    #[tokio::test]
    async fn middleware_injects_bearer_for_matching_host() {
        use reqwest_middleware::ClientBuilder;
        use std::sync::Arc;

        let server = axum::Router::new().route(
            "/check",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                headers
                    .get("authorization")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });

        let server_url = Url::parse(&format!("http://{addr}")).unwrap();
        let token = TrustedPublishingToken::new("abc123".to_string());
        let middleware = TrustedPublishingMiddleware::with_token(&server_url, token);

        let client = ClientBuilder::new(reqwest::Client::new())
            .with_arc(Arc::new(middleware))
            .build();

        let body = client
            .get(server_url.join("/check").unwrap())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "Bearer abc123");
    }

    #[tokio::test]
    async fn middleware_skips_same_host_different_channel() {
        use reqwest_middleware::ClientBuilder;
        use std::sync::Arc;

        let server = axum::Router::new()
            .route(
                "/my-channel/check",
                axum::routing::get(|headers: axum::http::HeaderMap| async move {
                    if headers.contains_key("authorization") {
                        "has-auth".to_string()
                    } else {
                        "no-auth".to_string()
                    }
                }),
            )
            .route(
                "/other-channel/check",
                axum::routing::get(|headers: axum::http::HeaderMap| async move {
                    if headers.contains_key("authorization") {
                        "has-auth".to_string()
                    } else {
                        "no-auth".to_string()
                    }
                }),
            )
            .route(
                "/my-channel-evil/check",
                axum::routing::get(|headers: axum::http::HeaderMap| async move {
                    if headers.contains_key("authorization") {
                        "has-auth".to_string()
                    } else {
                        "no-auth".to_string()
                    }
                }),
            )
            .route(
                "/my-channel/subdir/check",
                axum::routing::get(|headers: axum::http::HeaderMap| async move {
                    if headers.contains_key("authorization") {
                        "has-auth".to_string()
                    } else {
                        "no-auth".to_string()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });

        // Middleware scoped to /my-channel/ on the test host.
        let channel_url = Url::parse(&format!("http://{addr}/my-channel/")).unwrap();
        let token = TrustedPublishingToken::new("abc123".to_string());
        let middleware = TrustedPublishingMiddleware::with_token(&channel_url, token);

        let client = ClientBuilder::new(reqwest::Client::new())
            .with_arc(Arc::new(middleware))
            .build();

        // Same host, different channel: token must NOT be injected.
        let body = client
            .get(format!("http://{addr}/other-channel/check"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "no-auth");

        // Prefix collision: /my-channel-evil must NOT match /my-channel/.
        let body = client
            .get(format!("http://{addr}/my-channel-evil/check"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "no-auth");

        // Same channel: token IS injected.
        let body = client
            .get(format!("http://{addr}/my-channel/check"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "has-auth");

        // Sub-path under the same channel: token IS injected.
        let body = client
            .get(format!("http://{addr}/my-channel/subdir/check"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "has-auth");
    }

    #[test]
    fn normalize_channel_url_adds_trailing_slash() {
        let with_path = Url::parse("https://prefix.dev/my-channel").unwrap();
        assert_eq!(normalize_channel_url(&with_path).path(), "/my-channel/");

        let already_trailing = Url::parse("https://prefix.dev/my-channel/").unwrap();
        assert_eq!(
            normalize_channel_url(&already_trailing).path(),
            "/my-channel/"
        );

        // The url crate normalizes a host-only URL to path "/".
        let host_only = Url::parse("https://prefix.dev").unwrap();
        assert_eq!(host_only.path(), "/");
        assert_eq!(normalize_channel_url(&host_only).path(), "/");
    }

    #[tokio::test]
    async fn middleware_skips_non_matching_host() {
        use reqwest_middleware::ClientBuilder;
        use std::sync::Arc;

        let server = axum::Router::new().route(
            "/check",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                if headers.contains_key("authorization") {
                    "has-auth".to_string()
                } else {
                    "no-auth".to_string()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });

        // Middleware is configured for a different host than the one we hit.
        let other_url = Url::parse("https://example.invalid").unwrap();
        let token = TrustedPublishingToken::new("abc123".to_string());
        let middleware = TrustedPublishingMiddleware::with_token(&other_url, token);

        let client = ClientBuilder::new(reqwest::Client::new())
            .with_arc(Arc::new(middleware))
            .build();

        let body = client
            .get(format!("http://{addr}/check"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "no-auth");
    }
}
