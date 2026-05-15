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
    env::VarError,
    ffi::OsString,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use reqwest::StatusCode;
use reqwest_middleware::{ClientWithMiddleware, Middleware, Next};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

/// Refresh minted JWT tokens before they expire to avoid sending a token that
/// becomes invalid while a request is in flight.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// Environment-variable names used by the trusted-publishing flow. Kept
/// private — callers should not need to read these directly.
mod consts {
    pub const GITHUB_ACTIONS: &str = "GITHUB_ACTIONS";
    pub const ACTIONS_ID_TOKEN_REQUEST_URL: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";
    pub const ACTIONS_ID_TOKEN_REQUEST_TOKEN: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";

    pub const GITLAB_CI: &str = "GITLAB_CI";

    pub const CLOUD_BUILD_ID: &str = "CLOUD_BUILD_ID";
    pub const K_SERVICE: &str = "K_SERVICE";
    pub const GCE_METADATA_HOST: &str = "GCE_METADATA_HOST";
    pub const GCP_METADATA_HOST_DEFAULT: &str = "metadata.google.internal";
    pub const GCP_METADATA_IDENTITY_PATH: &str =
        "/computeMetadata/v1/instance/service-accounts/default/identity";
}

/// Default audience for the OIDC ID token. Matches prefix.dev's expectation.
pub const DEFAULT_AUDIENCE: &str = "prefix.dev";

/// Default path on the server where the OIDC ID token is exchanged for a
/// bearer token.
pub const DEFAULT_MINT_PATH: &str = "/api/oidc/mint_token";

/// Default name of the env var the user is expected to set in their
/// `.gitlab-ci.yml` `id_tokens` block.
pub const DEFAULT_GITLAB_ID_TOKEN_ENV: &str = "PREFIX_ID_TOKEN";

/// Knobs for the trusted-publishing flow. Defaults target prefix.dev; override
/// any field to point at a different server.
#[derive(Debug, Clone)]
pub struct TrustedPublishingOptions {
    /// The `aud` claim requested in the OIDC ID token. The server validates
    /// this against the trusted-publisher configuration before minting a
    /// token.
    pub audience: String,
    /// Path on the server (joined onto `server_url`) where the ID token is
    /// exchanged for a bearer token.
    pub mint_path: String,
    /// Name of the env var that GitLab CI populates via the `id_tokens` job
    /// keyword. Users have to configure this in `.gitlab-ci.yml`.
    pub gitlab_id_token_env: String,
}

impl Default for TrustedPublishingOptions {
    fn default() -> Self {
        Self {
            audience: DEFAULT_AUDIENCE.to_string(),
            mint_path: DEFAULT_MINT_PATH.to_string(),
            gitlab_id_token_env: DEFAULT_GITLAB_ID_TOKEN_ENV.to_string(),
        }
    }
}

/// Represents the CI provider being used for trusted publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiProvider {
    /// GitHub Actions.
    GitHubActions,
    /// GitLab CI.
    GitLabCI,
    /// Google Cloud (Cloud Build, Cloud Run, GCE, or GKE).
    GoogleCloud,
}

impl std::fmt::Display for CiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiProvider::GitHubActions => write!(f, "GitHub Actions"),
            CiProvider::GitLabCI => write!(f, "GitLab CI"),
            CiProvider::GoogleCloud => write!(f, "Google Cloud"),
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

/// Detects which CI provider is being used, if any.
pub fn detect_ci_provider() -> Option<CiProvider> {
    if github_action_runner() {
        Some(CiProvider::GitHubActions)
    } else if gitlab_ci_runner() {
        Some(CiProvider::GitLabCI)
    } else if google_cloud_runner() {
        Some(CiProvider::GoogleCloud)
    } else {
        None
    }
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
    /// A required CI environment variable was not set.
    #[error("Environment variable {0} not set, is the `id-token: write` permission missing?")]
    MissingEnvVar(&'static str),
    /// A required CI environment variable was set but not valid UTF-8.
    #[error("Environment variable {0} is not valid UTF-8: `{1:?}`")]
    InvalidEnvVar(&'static str, OsString),
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
    /// GitLab CI: the `id_tokens` env var was missing or empty.
    #[error(
        "GitLab CI OIDC token not found in env var `{0}`. Make sure you have configured `id_tokens` in your .gitlab-ci.yml:\n\n\
        job_name:\n  \
          id_tokens:\n    \
            {0}:\n      \
              aud: {1}\n"
    )]
    GitLabOidcTokenNotFound(String, String),
    /// Google Cloud: the metadata server did not return a token.
    #[error("Google Cloud OIDC token retrieval failed. Make sure you are running in a Google Cloud environment (Cloud Build, Cloud Run, GCE, or GKE) with a service account attached.")]
    GoogleCloudOidcTokenNotFound,
}

impl TrustedPublishingError {
    fn from_var_err(env_var: &'static str, err: VarError) -> Self {
        match err {
            VarError::NotPresent => Self::MissingEnvVar(env_var),
            VarError::NotUnicode(os_string) => Self::InvalidEnvVar(env_var, os_string),
        }
    }
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

/// The response from querying GitHub's OIDC endpoint
/// (`$ACTIONS_ID_TOKEN_REQUEST_URL&audience=...`).
#[derive(Deserialize)]
struct OidcToken {
    value: String,
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
    let provider = match detect_ci_provider() {
        Some(p) => p,
        None => return TrustedPublishResult::Skipped,
    };

    tracing::debug!(
        "Running on {} without explicit credentials, checking for trusted publishing",
        provider
    );

    match get_token(client, server_url, provider, options).await {
        Ok(token) => TrustedPublishResult::Configured(token),
        Err(err) => {
            tracing::debug!("Could not obtain trusted publishing credentials, skipping: {err}");
            TrustedPublishResult::Ignored(err)
        }
    }
}

/// Returns the short-lived token to use against `server_url`.
pub async fn get_token(
    client: &ClientWithMiddleware,
    server_url: &Url,
    provider: CiProvider,
    options: &TrustedPublishingOptions,
) -> Result<TrustedPublishingToken, TrustedPublishingError> {
    let oidc_token = match provider {
        CiProvider::GitHubActions => {
            let oidc_token_request_token = env::var(consts::ACTIONS_ID_TOKEN_REQUEST_TOKEN)
                .map_err(|err| {
                    TrustedPublishingError::from_var_err(
                        consts::ACTIONS_ID_TOKEN_REQUEST_TOKEN,
                        err,
                    )
                })?;
            get_github_oidc_token(&oidc_token_request_token, client, options).await?
        }
        CiProvider::GitLabCI => get_gitlab_oidc_token(options)?,
        CiProvider::GoogleCloud => get_google_cloud_oidc_token(client, options).await?,
    };

    let publish_token = get_publish_token(&oidc_token, server_url, client, options).await?;

    tracing::info!("Received token from {}, using trusted publishing", provider);

    // Mask the token in CI logs when the runner supports it.
    if provider == CiProvider::GitHubActions {
        println!("::add-mask::{}", publish_token.secret());
    }

    Ok(publish_token)
}

fn get_gitlab_oidc_token(
    options: &TrustedPublishingOptions,
) -> Result<String, TrustedPublishingError> {
    let env_name = options.gitlab_id_token_env.as_str();
    match env::var(env_name) {
        Ok(token) if !token.is_empty() => {
            tracing::info!("Found GitLab CI OIDC token in {env_name}");
            Ok(token)
        }
        Ok(_) => {
            tracing::warn!("{env_name} is set but empty");
            Err(TrustedPublishingError::GitLabOidcTokenNotFound(
                env_name.to_string(),
                options.audience.clone(),
            ))
        }
        Err(_) => {
            tracing::debug!("{env_name} not found in environment");
            Err(TrustedPublishingError::GitLabOidcTokenNotFound(
                env_name.to_string(),
                options.audience.clone(),
            ))
        }
    }
}

async fn get_github_oidc_token(
    oidc_token_request_token: &str,
    client: &ClientWithMiddleware,
    options: &TrustedPublishingOptions,
) -> Result<String, TrustedPublishingError> {
    let oidc_token_url = env::var(consts::ACTIONS_ID_TOKEN_REQUEST_URL).map_err(|err| {
        TrustedPublishingError::from_var_err(consts::ACTIONS_ID_TOKEN_REQUEST_URL, err)
    })?;
    let mut oidc_token_url = Url::parse(&oidc_token_url)?;
    oidc_token_url
        .query_pairs_mut()
        .append_pair("audience", &options.audience);
    tracing::info!("Querying the trusted publishing OIDC token from {oidc_token_url}");
    let response = client
        .get(oidc_token_url.clone())
        .bearer_auth(oidc_token_request_token)
        .send()
        .await
        .map_err(|err| TrustedPublishingError::ReqwestMiddleware(oidc_token_url.clone(), err))?;
    let oidc_token: OidcToken = response
        .error_for_status()
        .map_err(|err| TrustedPublishingError::Reqwest(oidc_token_url.clone(), err))?
        .json()
        .await
        .map_err(|err| TrustedPublishingError::Reqwest(oidc_token_url.clone(), err))?;
    Ok(oidc_token.value)
}

async fn get_google_cloud_oidc_token(
    client: &ClientWithMiddleware,
    options: &TrustedPublishingOptions,
) -> Result<String, TrustedPublishingError> {
    let metadata_host = env::var(consts::GCE_METADATA_HOST)
        .unwrap_or_else(|_| consts::GCP_METADATA_HOST_DEFAULT.to_string());

    let metadata_url = format!(
        "http://{}{}?audience={}",
        metadata_host,
        consts::GCP_METADATA_IDENTITY_PATH,
        options.audience,
    );
    let url = Url::parse(&metadata_url)?;

    tracing::info!(
        "Querying the trusted publishing OIDC token from Google Cloud metadata server at {}",
        metadata_host
    );

    let response = client
        .get(url.clone())
        .header("Metadata-Flavor", "Google")
        .send()
        .await
        .map_err(|err| TrustedPublishingError::ReqwestMiddleware(url.clone(), err))?;

    if !response.status().is_success() {
        tracing::warn!(
            "Google Cloud metadata server returned status {}",
            response.status()
        );
        return Err(TrustedPublishingError::GoogleCloudOidcTokenNotFound);
    }

    let token = response
        .text()
        .await
        .map_err(|err| TrustedPublishingError::Reqwest(url.clone(), err))?;

    if token.is_empty() {
        tracing::warn!("Google Cloud metadata server returned empty token");
        return Err(TrustedPublishingError::GoogleCloudOidcTokenNotFound);
    }

    tracing::info!("Successfully obtained OIDC token from Google Cloud");
    Ok(token)
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
        {
            if let Some(token) = self.token().await {
                let bearer_auth = format!("Bearer {}", token.secret());
                let mut header_value = reqwest::header::HeaderValue::from_str(&bearer_auth)
                    .map_err(reqwest_middleware::Error::middleware)?;
                header_value.set_sensitive(true);
                req.headers_mut()
                    .insert(reqwest::header::AUTHORIZATION, header_value);
            }
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
        assert_eq!(opts.gitlab_id_token_env, "PREFIX_ID_TOKEN");
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
                let auth = headers
                    .get("authorization")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default();
                auth
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
