//! Host-scoped middleware that reacts to `WWW-Authenticate` challenges by
//! acquiring a bearer token from a pluggable [`AuthFlow`] and replaying the
//! request once.
//!
//! The first [`AuthFlow`] implementation is
//! [`crate::trusted_publishing::TrustedPublishingFlow`].

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use reqwest_middleware::{Middleware, Next};
use serde::Deserialize;
use thiserror::Error;
use url::Url;

/// One parsed challenge from a `WWW-Authenticate` response header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    /// The authentication scheme, e.g. `Bearer` (case preserved as sent).
    pub scheme: String,
    /// Auth parameters with lowercased keys, e.g. `realm` -> `prefix.dev`.
    /// `token68` payloads (e.g. base64 blobs after the scheme) are skipped.
    pub params: HashMap<String, String>,
}

/// Parse all challenges from every `WWW-Authenticate` header in `headers`.
///
/// Tolerant by design: malformed input yields fewer (or no) challenges,
/// never an error or panic. Handles multiple comma-separated challenges in
/// one header value as well as the header appearing multiple times.
pub fn parse_challenges(headers: &http::HeaderMap) -> Vec<Challenge> {
    headers
        .get_all(http::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(parse_header_value)
        .collect()
}

/// An auth scheme is a token of ASCII alphanumerics plus a few safe symbols.
/// Stricter than RFC 7235's `token` on purpose: it rejects line noise that
/// would otherwise be misread as a scheme.
fn is_valid_scheme(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn parse_header_value(value: &str) -> Vec<Challenge> {
    let mut challenges: Vec<Challenge> = Vec::new();
    for item in split_commas_respecting_quotes(value) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        // A new challenge starts with a scheme token; a continuation item is
        // a bare `key=value` auth-param belonging to the current challenge.
        let (first, rest) = match item.split_once(char::is_whitespace) {
            Some((first, rest)) => (first, Some(rest.trim())),
            None => (item, None),
        };
        if !first.contains('=') {
            if !is_valid_scheme(first) {
                continue;
            }
            challenges.push(Challenge {
                scheme: first.to_string(),
                params: HashMap::new(),
            });
            if let (Some(rest), Some(challenge)) = (rest, challenges.last_mut())
                && let Some((key, val)) = parse_param(rest)
            {
                challenge.params.insert(key, val);
            }
        } else if let Some(challenge) = challenges.last_mut()
            && let Some((key, val)) = parse_param(item)
        {
            challenge.params.insert(key, val);
        }
    }
    challenges
}

/// Parse one `key=value` or `key="quoted value"` auth-param. Returns `None`
/// for non-params (e.g. token68 blobs like `YII=`, which have an empty
/// "value" after the trailing `=`).
fn parse_param(s: &str) -> Option<(String, String)> {
    let (key, value) = s.split_once('=')?;
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim();
    if key.is_empty() || value.is_empty() || !is_valid_scheme(&key) {
        return None;
    }
    // A value consisting only of `=` characters is padding from a token68
    // blob (e.g. `dGVzdA==` splits into key `dGVzdA`, value `=`). Skip it.
    if value.chars().all(|c| c == '=') {
        return None;
    }
    let value = if let Some(rest) = value.strip_prefix('"') {
        // Quoted string: require a clean closing quote with nothing after it
        // and no unescaped quotes inside; anything else (unterminated, or
        // trailing garbage like a second space-separated param) is malformed
        // and yields no param rather than a wrong value.
        let inner = rest.strip_suffix('"')?;
        if malformed_quoted_interior(inner) {
            return None;
        }
        inner.replace("\\\"", "\"")
    } else {
        // Unquoted token: any stray quote means a mangled quoted string.
        if value.contains('"') {
            return None;
        }
        value.to_string()
    };
    Some((key, value))
}

/// Returns `true` when `s` (the interior of a quoted string, outer quotes
/// already stripped) contains an unescaped `"` — e.g. two space-separated
/// quoted params mashed into one value — or ends with a dangling escape,
/// which means the stripped closing quote was actually escaped (i.e. the
/// quoted string was never terminated).
fn malformed_quoted_interior(s: &str) -> bool {
    let mut escaped = false;
    for c in s.chars() {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return true,
            _ => escaped = false,
        }
    }
    escaped
}

/// Split on commas that are not inside a double-quoted string.
fn split_commas_respecting_quotes(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match c {
            '\\' if in_quotes && !escaped => escaped = true,
            '"' if !escaped => {
                in_quotes = !in_quotes;
                escaped = false;
            }
            ',' if !in_quotes => {
                parts.push(&s[start..i]);
                start = i + 1;
                escaped = false;
            }
            _ => escaped = false,
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Refresh tokens this long before their `exp` so a token does not become
/// invalid while a request is in flight.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// A short-lived bearer token acquired by an [`AuthFlow`] implementation.
///
/// `Deserialize`-transparent (a raw JSON string body deserializes directly
/// into it) and `Clone` so it can be shared between the cache and requests.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct BearerToken(String);

impl BearerToken {
    /// Wrap an existing token string.
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// The raw bearer token. Treat as sensitive; don't log it.
    pub fn secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BearerToken").field(&"<redacted>").finish()
    }
}

/// Error produced by an [`AuthFlow`] implementation.
///
/// Boxed so custom flows can surface arbitrary failures. The middleware only
/// logs this error and disables further attempts — it never propagates it,
/// so the caller always observes the server's original response.
#[derive(Debug, Error)]
#[error("authentication flow failed: {source}")]
pub struct AuthFlowError {
    #[source]
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl AuthFlowError {
    /// Wrap any error produced by an [`AuthFlow`] implementation.
    pub fn new(err: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>) -> Self {
        Self { source: err.into() }
    }
}

/// A pluggable strategy that turns a `WWW-Authenticate` challenge into a
/// bearer token.
///
/// Implementations decide which challenges they support (e.g. only scheme
/// `Bearer`) and how to acquire the token (OIDC exchange, device flow, ...).
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait AuthFlow: Send + Sync + fmt::Debug {
    /// Respond to `challenges` received from `url`.
    ///
    /// Return `Ok(None)` when this flow does not apply (e.g. unsupported
    /// scheme, or not running in a CI environment) — the middleware caches
    /// that negatively and stops asking for the lifetime of the process.
    ///
    /// `url` is the full URL of the request that was challenged (path and
    /// query included), not just the server origin. The flow may be invoked
    /// concurrently by parallel requests racing on the first challenge;
    /// implementations should tolerate redundant invocations (every returned
    /// token is cached last-write-wins).
    ///
    /// Implementations may treat `url`'s origin as a trusted credential sink;
    /// dispatchers must therefore only invoke flows for URLs on the host they
    /// are scoped to.
    async fn acquire_token(
        &self,
        url: &Url,
        challenges: &[Challenge],
    ) -> Result<Option<BearerToken>, AuthFlowError>;
}

#[derive(Deserialize)]
struct JwtClaims {
    exp: Option<u64>,
}

/// Best-effort extraction of the `exp` claim from a JWT-shaped token.
/// Returns `None` for opaque tokens, which are then cached without expiry.
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

#[derive(Debug)]
struct CachedToken {
    /// Pre-validated `Bearer <secret>` header value, marked sensitive.
    header: reqwest::header::HeaderValue,
    expires_at: Option<SystemTime>,
}

impl CachedToken {
    /// Fails when the token cannot be encoded as an HTTP header value —
    /// callers must treat that like a failed acquisition, not cache it.
    fn new(token: &BearerToken) -> Result<Self, reqwest::header::InvalidHeaderValue> {
        let mut header =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token.secret()))?;
        header.set_sensitive(true);
        Ok(Self {
            header,
            expires_at: jwt_expiration(token.secret()),
        })
    }

    fn is_fresh(&self, now: SystemTime) -> bool {
        self.expires_at
            .is_none_or(|expires_at| now + TOKEN_REFRESH_MARGIN < expires_at)
    }
}

/// Cache state shared by all clones of one middleware instance.
#[derive(Debug, Default)]
enum TokenCache {
    /// No acquisition attempted yet.
    #[default]
    Empty,
    /// The flow reported "not applicable" or failed — stop asking.
    Disabled,
    /// A previously acquired token.
    Token(CachedToken),
}

/// Outcome of a cache lookup, decoupled from the lock.
enum CacheLookup {
    Empty,
    Disabled,
    Fresh(reqwest::header::HeaderValue),
}

/// Host-scoped `reqwest` middleware that acquires a bearer token via an
/// [`AuthFlow`] when (and only when) the server answers with a
/// `WWW-Authenticate` challenge, then replays the request once.
///
/// Construct one instance per server. A request is in scope when its scheme,
/// host, and effective port match `server`; the path is ignored. Requests
/// that already carry an `Authorization` header are never touched, so
/// credentials from [`crate::AuthenticationMiddleware`] always win.
/// (Credentials embedded in the URL path or query — e.g. conda `/t/<token>`
/// tokens — are not detected; a challenged request carrying such credentials
/// will still be replayed with a bearer token.)
///
/// The first acquired token is cached (with JWT-expiry-aware refresh); a flow
/// that reports "not applicable" or fails disables the middleware for the
/// process lifetime. Acquisition failures are logged, never propagated: the
/// caller then observes the server's original 401/403 response.
#[derive(Clone, Debug)]
pub struct AuthChallengeMiddleware {
    server: Url,
    flow: Arc<dyn AuthFlow>,
    cache: Arc<Mutex<TokenCache>>,
}

impl AuthChallengeMiddleware {
    /// Create a middleware guarding the server identified by `server`'s
    /// scheme, host, and port. `server`'s path is ignored.
    pub fn new(server: Url, flow: Arc<dyn AuthFlow>) -> Self {
        Self {
            server,
            flow,
            cache: Arc::new(Mutex::new(TokenCache::Empty)),
        }
    }

    fn matches_host(&self, url: &Url) -> bool {
        url.scheme() == self.server.scheme()
            && url.host_str() == self.server.host_str()
            && url.port_or_known_default() == self.server.port_or_known_default()
    }

    fn lookup_cache(&self) -> CacheLookup {
        let cache = self
            .cache
            .lock()
            .expect("auth challenge token cache poisoned");
        match &*cache {
            TokenCache::Disabled => CacheLookup::Disabled,
            TokenCache::Token(cached) if cached.is_fresh(SystemTime::now()) => {
                CacheLookup::Fresh(cached.header.clone())
            }
            TokenCache::Empty | TokenCache::Token(_) => CacheLookup::Empty,
        }
    }

    /// Run the flow and record the outcome. `Ok(None)` and errors both
    /// disable the middleware; errors are additionally logged.
    async fn acquire_and_cache(
        &self,
        url: &Url,
        challenges: &[Challenge],
    ) -> Option<reqwest::header::HeaderValue> {
        let result = self.flow.acquire_token(url, challenges).await;
        let mut cache = self
            .cache
            .lock()
            .expect("auth challenge token cache poisoned");
        match result {
            Ok(Some(token)) => match CachedToken::new(&token) {
                Ok(cached) => {
                    let header = cached.header.clone();
                    *cache = TokenCache::Token(cached);
                    Some(header)
                }
                Err(err) => {
                    tracing::warn!(
                        "AuthChallengeMiddleware: flow returned a token for {url} that is \
                         not a valid header value ({err}), disabling"
                    );
                    *cache = TokenCache::Disabled;
                    None
                }
            },
            Ok(None) => {
                tracing::debug!(
                    "AuthChallengeMiddleware: flow not applicable for {url}, disabling"
                );
                *cache = TokenCache::Disabled;
                None
            }
            Err(err) => {
                tracing::warn!("AuthChallengeMiddleware: failed to acquire token for {url}: {err}");
                *cache = TokenCache::Disabled;
                None
            }
        }
    }
}

fn attach_bearer(req: &mut reqwest::Request, header: reqwest::header::HeaderValue) {
    req.headers_mut()
        .insert(reqwest::header::AUTHORIZATION, header);
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Middleware for AuthChallengeMiddleware {
    async fn handle(
        &self,
        mut req: reqwest::Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        if !self.matches_host(req.url())
            || req.headers().contains_key(reqwest::header::AUTHORIZATION)
        {
            return next.run(req, extensions).await;
        }

        let cached = self.lookup_cache();
        if matches!(cached, CacheLookup::Disabled) {
            return next.run(req, extensions).await;
        }

        let used_cached_token = if let CacheLookup::Fresh(header) = cached {
            attach_bearer(&mut req, header);
            true
        } else {
            false
        };

        // Clone before sending so we can replay on a challenge. Fails only
        // for streaming bodies (absent on the GET-only read path) — then the
        // response is passed through unmodified.
        let retry_req = req.try_clone();
        let url = req.url().clone();
        let response = next.clone().run(req, extensions).await?;

        let challenges = parse_challenges(response.headers());
        if challenges.is_empty() {
            return Ok(response);
        }
        let Some(mut retry_req) = retry_req else {
            return Ok(response);
        };

        if used_cached_token {
            // The server rejected a token we believed fresh (revoked early).
            // Drop it — and its header on the clone — before re-acquiring.
            *self
                .cache
                .lock()
                .expect("auth challenge token cache poisoned") = TokenCache::Empty;
            retry_req
                .headers_mut()
                .remove(reqwest::header::AUTHORIZATION);
        }

        let Some(header) = self.acquire_and_cache(&url, &challenges).await else {
            return Ok(response);
        };
        attach_bearer(&mut retry_req, header);
        // Replay exactly once; the replayed response is returned as-is even
        // if it is another challenge.
        next.run(retry_req, extensions).await
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, UNIX_EPOCH},
    };

    use reqwest_middleware::ClientBuilder;

    use super::*;

    /// [`AuthFlow`] returning a fixed answer; counts invocations.
    #[derive(Debug)]
    struct StaticFlow {
        token: Option<&'static str>,
        calls: AtomicUsize,
    }

    impl StaticFlow {
        fn new(token: Option<&'static str>) -> Arc<Self> {
            Arc::new(Self {
                token,
                calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait::async_trait]
    impl AuthFlow for StaticFlow {
        async fn acquire_token(
            &self,
            _url: &Url,
            _challenges: &[Challenge],
        ) -> Result<Option<BearerToken>, AuthFlowError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.token.map(|t| BearerToken::new(t.to_string())))
        }
    }

    /// Axum server: requires `Bearer <accept>` on /channel/repodata.json,
    /// answers 401 + WWW-Authenticate otherwise. Counts every request.
    async fn spawn_protected_server(accept: &'static str, hits: Arc<AtomicUsize>) -> Url {
        use axum::{http::StatusCode, response::IntoResponse, routing::get};
        let router = axum::Router::new().route(
            "/channel/repodata.json",
            get(move |headers: axum::http::HeaderMap| {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let expected = format!("Bearer {accept}");
                    match headers.get("authorization").and_then(|v| v.to_str().ok()) {
                        Some(auth) if auth == expected => (StatusCode::OK, "ok").into_response(),
                        _ => (
                            StatusCode::UNAUTHORIZED,
                            [("www-authenticate", r#"Bearer realm="test""#)],
                            "unauthorized",
                        )
                            .into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Url::parse(&format!("http://{addr}")).unwrap()
    }

    fn client_with(
        middleware: AuthChallengeMiddleware,
    ) -> reqwest_middleware::ClientWithMiddleware {
        ClientBuilder::new(reqwest::Client::new())
            .with_arc(Arc::new(middleware))
            .build()
    }

    #[tokio::test]
    async fn challenge_triggers_mint_and_replay() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let flow = StaticFlow::new(Some("abc123"));
        let client = client_with(AuthChallengeMiddleware::new(
            server_url.clone(),
            flow.clone(),
        ));

        let response = client
            .get(server_url.join("/channel/repodata.json").unwrap())
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 1);
        // one challenged request + one replay
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn second_request_reuses_cached_token_without_challenge() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let flow = StaticFlow::new(Some("abc123"));
        let client = client_with(AuthChallengeMiddleware::new(
            server_url.clone(),
            flow.clone(),
        ));

        let url = server_url.join("/channel/repodata.json").unwrap();
        assert_eq!(client.get(url.clone()).send().await.unwrap().status(), 200);
        assert_eq!(client.get(url).send().await.unwrap().status(), 200);

        // flow consulted exactly once; second request went straight through
        assert_eq!(flow.calls.load(Ordering::SeqCst), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn inapplicable_flow_is_negative_cached() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let flow = StaticFlow::new(None);
        let client = client_with(AuthChallengeMiddleware::new(
            server_url.clone(),
            flow.clone(),
        ));

        let url = server_url.join("/channel/repodata.json").unwrap();
        assert_eq!(client.get(url.clone()).send().await.unwrap().status(), 401);
        assert_eq!(client.get(url).send().await.unwrap().status(), 401);

        // flow consulted once, then disabled
        assert_eq!(flow.calls.load(Ordering::SeqCst), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    fn header_map(values: &[&str]) -> http::HeaderMap {
        let mut headers = http::HeaderMap::new();
        for v in values {
            headers.append(
                http::header::WWW_AUTHENTICATE,
                http::HeaderValue::from_str(v).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn parses_single_bearer_challenge() {
        let challenges = parse_challenges(&header_map(&[r#"Bearer realm="prefix.dev""#]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].scheme, "Bearer");
        assert_eq!(challenges[0].params["realm"], "prefix.dev");
    }

    #[test]
    fn parses_multiple_challenges_in_one_header() {
        let challenges = parse_challenges(&header_map(&[
            r#"Bearer realm="prefix.dev", error="invalid_token", Basic realm="other""#,
        ]));
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].scheme, "Bearer");
        assert_eq!(challenges[0].params["realm"], "prefix.dev");
        assert_eq!(challenges[0].params["error"], "invalid_token");
        assert_eq!(challenges[1].scheme, "Basic");
        assert_eq!(challenges[1].params["realm"], "other");
    }

    #[test]
    fn parses_multiple_headers() {
        let challenges =
            parse_challenges(&header_map(&[r#"Bearer realm="a""#, r#"Basic realm="b""#]));
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].scheme, "Bearer");
        assert_eq!(challenges[1].scheme, "Basic");
    }

    #[test]
    fn quoted_commas_do_not_split_challenges() {
        let challenges = parse_challenges(&header_map(&[r#"Bearer realm="a,b""#]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].params["realm"], "a,b");
    }

    #[test]
    fn unquoted_params_and_case_insensitive_keys() {
        let challenges = parse_challenges(&header_map(&["Bearer REALM=prefix.dev"]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].params["realm"], "prefix.dev");
    }

    #[test]
    fn token68_payload_is_skipped_not_a_param() {
        // e.g. `Negotiate YII=` — the trailing blob is not a key=value param
        let challenges = parse_challenges(&header_map(&["Negotiate YII="]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].scheme, "Negotiate");
        assert!(challenges[0].params.is_empty());
    }

    #[test]
    fn space_separated_params_yield_no_wrong_values() {
        // Non-RFC space-separated params: scheme parsed, malformed param dropped
        let challenges = parse_challenges(&header_map(&[
            r#"Bearer realm="prefix.dev" error="invalid_token""#,
        ]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].scheme, "Bearer");
        assert!(challenges[0].params.is_empty());
    }

    #[test]
    fn unbalanced_quote_yields_no_param() {
        let challenges = parse_challenges(&header_map(&[r#"Bearer realm="unterminated"#]));
        assert_eq!(challenges.len(), 1);
        assert!(challenges[0].params.is_empty());
    }

    #[test]
    fn garbage_yields_no_challenges_and_no_panic() {
        assert!(parse_challenges(&header_map(&["= = ="])).is_empty());
        assert!(parse_challenges(&header_map(&[",,,"])).is_empty());
        assert!(parse_challenges(&header_map(&[""])).is_empty());
        assert!(parse_challenges(&header_map(&["%%% ###"])).is_empty());
        assert!(parse_challenges(&http::HeaderMap::new()).is_empty());
    }

    #[test]
    fn bearer_token_debug_is_redacted() {
        let token = BearerToken::new("supersecret".to_string());
        let formatted = format!("{token:?}");
        assert!(!formatted.contains("supersecret"));
        assert!(formatted.contains("redacted"));
    }

    fn unsigned_jwt_with_exp(exp: u64) -> String {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
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
    fn opaque_token_has_no_expiration() {
        assert_eq!(jwt_expiration("not-a-jwt"), None);
    }

    #[test]
    fn cached_jwt_is_stale_inside_refresh_margin() {
        let token = BearerToken::new(unsigned_jwt_with_exp(1_700_000_000));
        let cached = CachedToken::new(&token).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000 - 30);
        assert!(!cached.is_fresh(now));
        let earlier = UNIX_EPOCH + Duration::from_secs(1_700_000_000 - 3600);
        assert!(cached.is_fresh(earlier));
    }

    #[tokio::test]
    async fn header_invalid_token_is_not_cached_and_does_not_error() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let flow = StaticFlow::new(Some("bad\ntoken"));
        let client = client_with(AuthChallengeMiddleware::new(
            server_url.clone(),
            flow.clone(),
        ));
        let url = server_url.join("/channel/repodata.json").unwrap();

        // The caller sees the server's original 401, not a middleware error,
        // and the middleware disables itself instead of caching the bad token.
        assert_eq!(client.get(url.clone()).send().await.unwrap().status(), 401);
        assert_eq!(client.get(url).send().await.unwrap().status(), 401);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 1);
    }

    /// [`AuthFlow`] yielding a different token per call (for the stale-token test).
    #[derive(Debug)]
    struct SequenceFlow {
        tokens: Mutex<Vec<&'static str>>,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl AuthFlow for SequenceFlow {
        async fn acquire_token(
            &self,
            _url: &Url,
            _challenges: &[Challenge],
        ) -> Result<Option<BearerToken>, AuthFlowError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut tokens = self.tokens.lock().unwrap();
            assert!(
                !tokens.is_empty(),
                "SequenceFlow exhausted: middleware called acquire_token more times than expected"
            );
            let token = tokens.remove(0);
            Ok(Some(BearerToken::new(token.to_string())))
        }
    }

    /// [`AuthFlow`] that always fails.
    #[derive(Debug)]
    struct FailingFlow {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl AuthFlow for FailingFlow {
        async fn acquire_token(
            &self,
            _url: &Url,
            _challenges: &[Challenge],
        ) -> Result<Option<BearerToken>, AuthFlowError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(AuthFlowError::new(std::io::Error::other("mint exploded")))
        }
    }

    #[tokio::test]
    async fn non_matching_host_is_untouched() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let flow = StaticFlow::new(Some("abc123"));
        let other_host = Url::parse("https://example.invalid").unwrap();
        let client = client_with(AuthChallengeMiddleware::new(other_host, flow.clone()));

        let response = client
            .get(server_url.join("/channel/repodata.json").unwrap())
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 401);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 0);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn scheme_mismatch_is_untouched() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        // same host and port, but https configured vs the http test server
        let mut https_url = server_url.clone();
        https_url.set_scheme("https").unwrap();
        let flow = StaticFlow::new(Some("abc123"));
        let client = client_with(AuthChallengeMiddleware::new(https_url, flow.clone()));

        let response = client
            .get(server_url.join("/channel/repodata.json").unwrap())
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 401);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 0);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn existing_authorization_header_is_respected() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let flow = StaticFlow::new(Some("abc123"));
        let client = client_with(AuthChallengeMiddleware::new(
            server_url.clone(),
            flow.clone(),
        ));

        let response = client
            .get(server_url.join("/channel/repodata.json").unwrap())
            .header(reqwest::header::AUTHORIZATION, "Bearer user-supplied")
            .send()
            .await
            .unwrap();

        // wrong credentials stay wrong: no override, no replay
        assert_eq!(response.status(), 401);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 0);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn replays_at_most_once() {
        let hits = Arc::new(AtomicUsize::new(0));
        // server accepts a token the flow never produces -> always 401
        let server_url = spawn_protected_server("never-issued", hits.clone()).await;
        let flow = StaticFlow::new(Some("abc123"));
        let client = client_with(AuthChallengeMiddleware::new(
            server_url.clone(),
            flow.clone(),
        ));

        let response = client
            .get(server_url.join("/channel/repodata.json").unwrap())
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 401);
        // initial request + exactly one replay, nothing more
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stale_cached_token_is_cleared_and_reacquired() {
        use axum::{http::StatusCode, response::IntoResponse, routing::get};

        // Server accepting only "Bearer fresh", recording the Authorization
        // header of every request it sees.
        let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_in_handler = seen.clone();
        let router = axum::Router::new().route(
            "/channel/repodata.json",
            get(move |headers: axum::http::HeaderMap| {
                let seen = seen_in_handler.clone();
                async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    seen.lock().unwrap().push(auth.clone());
                    if auth.as_deref() == Some("Bearer fresh") {
                        (StatusCode::OK, "ok").into_response()
                    } else {
                        (
                            StatusCode::UNAUTHORIZED,
                            [("www-authenticate", r#"Bearer realm="test""#)],
                            "unauthorized",
                        )
                            .into_response()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let server_url = Url::parse(&format!("http://{addr}")).unwrap();

        let flow = Arc::new(SequenceFlow {
            tokens: Mutex::new(vec!["old", "fresh"]),
            calls: AtomicUsize::new(0),
        });
        let client = client_with(AuthChallengeMiddleware::new(
            server_url.clone(),
            flow.clone(),
        ));
        let url = server_url.join("/channel/repodata.json").unwrap();

        // Request 1: challenge -> flow mints "old" (cached before the replay
        // proves it stale) -> replay rejected (401).
        assert_eq!(client.get(url.clone()).send().await.unwrap().status(), 401);

        // Request 2: cached "old" attached -> challenged -> cache cleared ->
        // flow mints "fresh" -> replay succeeds.
        assert_eq!(client.get(url).send().await.unwrap().status(), 200);

        assert_eq!(flow.calls.load(Ordering::SeqCst), 2);
        // The header sequence proves the cached path: request 2's first leg
        // carried the cached "old" token (a non-caching implementation would
        // send no header there).
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                None,
                Some("Bearer old".to_string()),
                Some("Bearer old".to_string()),
                Some("Bearer fresh".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn flow_error_is_swallowed_and_negative_cached() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let flow = Arc::new(FailingFlow {
            calls: AtomicUsize::new(0),
        });
        let client = client_with(AuthChallengeMiddleware::new(
            server_url.clone(),
            flow.clone(),
        ));
        let url = server_url.join("/channel/repodata.json").unwrap();

        // caller sees the server's 401, not the flow error
        assert_eq!(client.get(url.clone()).send().await.unwrap().status(), 401);
        assert_eq!(client.get(url).send().await.unwrap().status(), 401);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 1);
        // Disabled = pass-through (next.run is still called), so both requests
        // reach the server: request 1 triggers the challenge (1 hit), request 2
        // is passed through unmodified and also reaches the server (1 hit).
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn token68_with_double_padding_is_skipped() {
        let challenges = parse_challenges(&header_map(&["Negotiate dGVzdA=="]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].scheme, "Negotiate");
        assert!(challenges[0].params.is_empty());
    }
}
