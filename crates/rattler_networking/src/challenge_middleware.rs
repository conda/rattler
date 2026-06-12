//! Middleware that reacts to `WWW-Authenticate` challenges by acquiring a
//! bearer token from one of its registered [`AuthFlow`]s and replaying the
//! request once. Tokens are cached per origin (scheme, host, port).
//!
//! [`AuthChallengeMiddleware::default`] registers
//! [`crate::trusted_publishing::PrefixAuthAmbientFlow`] for zero-config
//! prefix.dev auth from CI.

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
/// Tolerant: malformed input yields fewer challenges, never an error or
/// panic. Handles multiple challenges per header and repeated headers.
pub fn parse_challenges(headers: &http::HeaderMap) -> Vec<Challenge> {
    headers
        .get_all(http::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(parse_header_value)
        .collect()
}

/// Schemes are ASCII alphanumerics plus `-`, `_`, `.`. Stricter than RFC
/// 7235's `token` to avoid misreading line noise as a scheme.
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
        // Quoted string: require a clean closing quote and no unescaped
        // quotes inside; anything else yields no param rather than a
        // wrong value.
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

/// True when the quoted-string interior `s` (outer quotes stripped)
/// contains an unescaped `"` or ends with a dangling escape, i.e. the
/// stripped closing quote was escaped and the string never terminated.
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

/// A short-lived bearer token acquired by an [`AuthFlow`].
///
/// `Deserialize`-transparent: a raw JSON string deserializes into it.
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

/// Error produced by an [`AuthFlow`].
///
/// Boxed so custom flows can surface arbitrary failures. The middleware
/// logs it and never propagates it; the caller observes the server's
/// original response.
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
    /// Return `Ok(None)` when the flow does not apply (unsupported scheme,
    /// not in CI, untrusted origin); the origin is then negative-cached.
    /// `url` is the full challenged URL, not just the origin. Flows may be
    /// invoked concurrently; redundant invocations must be tolerated
    /// (tokens are cached last-write-wins).
    ///
    /// The middleware invokes flows for every challenged URL, so flows
    /// that forward credentials MUST validate `url`'s origin themselves;
    /// see [`crate::trusted_publishing::PrefixAuthAmbientFlow`] for the
    /// pattern.
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
    /// Fails when the token cannot be encoded as a header value; callers
    /// must treat that as a failed acquisition, not cache it.
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

/// Cache state for one origin.
#[derive(Debug, Default)]
enum TokenCache {
    /// No acquisition attempted yet.
    #[default]
    Empty,
    /// Every flow declined or failed; stop asking.
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

/// Cache key: (scheme, host, effective port). `None` for URLs without a
/// host or known port (`data:`, `file:`), which pass through untouched.
type OriginKey = (String, String, u16);

fn origin_key(url: &Url) -> Option<OriginKey> {
    Some((
        url.scheme().to_string(),
        url.host_str()?.to_string(),
        url.port_or_known_default()?,
    ))
}

/// `reqwest` middleware that reacts to a `WWW-Authenticate` challenge by
/// acquiring a bearer token from its registered [`AuthFlow`]s (consulted
/// in order, first token wins) and replaying the request once.
///
/// Flows gate their own origins (see [`AuthFlow`]), so one instance serves
/// every host. Requests already carrying `Authorization` are never
/// touched: credentials from [`crate::AuthenticationMiddleware`] win.
/// Credentials in the URL path or query (conda `/t/<token>`) are not
/// detected; such requests are still replayed with a bearer token.
///
/// Tokens are cached per origin (scheme, host, effective port) with
/// JWT-expiry-aware refresh; a token for one origin is never replayed to
/// another. An origin every flow declines is disabled for the process
/// lifetime. Flow failures are logged, never propagated: the caller
/// observes the server's original 401/403 response.
///
/// [`Self::default`] registers
/// [`crate::trusted_publishing::PrefixAuthAmbientFlow`] for zero-config
/// prefix.dev auth from CI.
#[derive(Clone, Debug)]
pub struct AuthChallengeMiddleware {
    flows: Vec<Arc<dyn AuthFlow>>,
    caches: Arc<Mutex<HashMap<OriginKey, TokenCache>>>,
}

impl Default for AuthChallengeMiddleware {
    fn default() -> Self {
        Self::new(vec![Arc::new(
            crate::trusted_publishing::PrefixAuthAmbientFlow::default(),
        )])
    }
}

impl AuthChallengeMiddleware {
    /// Create a middleware consulting `flows` in order on every challenge.
    /// Flows gate their own origins (see [`AuthFlow::acquire_token`]).
    pub fn new(flows: Vec<Arc<dyn AuthFlow>>) -> Self {
        Self {
            flows,
            caches: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn lookup_cache(&self, origin: &OriginKey) -> CacheLookup {
        let caches = self
            .caches
            .lock()
            .expect("auth challenge token cache poisoned");
        match caches.get(origin) {
            Some(TokenCache::Disabled) => CacheLookup::Disabled,
            Some(TokenCache::Token(cached)) if cached.is_fresh(SystemTime::now()) => {
                CacheLookup::Fresh(cached.header.clone())
            }
            None | Some(TokenCache::Empty | TokenCache::Token(_)) => CacheLookup::Empty,
        }
    }

    fn store_cache(&self, origin: OriginKey, state: TokenCache) {
        self.caches
            .lock()
            .expect("auth challenge token cache poisoned")
            .insert(origin, state);
    }

    /// Consult flows in order and cache the outcome for `origin`. No
    /// usable token from any flow disables the origin; failures are
    /// logged.
    async fn acquire_and_cache(
        &self,
        origin: OriginKey,
        url: &Url,
        challenges: &[Challenge],
    ) -> Option<reqwest::header::HeaderValue> {
        for flow in &self.flows {
            match flow.acquire_token(url, challenges).await {
                Ok(Some(token)) => match CachedToken::new(&token) {
                    Ok(cached) => {
                        let header = cached.header.clone();
                        self.store_cache(origin, TokenCache::Token(cached));
                        return Some(header);
                    }
                    Err(err) => {
                        tracing::warn!(
                            "AuthChallengeMiddleware: {flow:?} returned a token for {url} \
                             that is not a valid header value ({err}), trying next flow"
                        );
                    }
                },
                Ok(None) => {
                    tracing::debug!(
                        "AuthChallengeMiddleware: {flow:?} not applicable for {url}, \
                         trying next flow"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        "AuthChallengeMiddleware: {flow:?} failed to acquire a token \
                         for {url}: {err}, trying next flow"
                    );
                }
            }
        }
        tracing::debug!(
            "AuthChallengeMiddleware: no flow produced a token for {url}, \
             disabling its origin"
        );
        self.store_cache(origin, TokenCache::Disabled);
        None
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
        let origin = origin_key(req.url());
        let Some(origin) = origin else {
            return next.run(req, extensions).await;
        };
        if req.headers().contains_key(reqwest::header::AUTHORIZATION) {
            return next.run(req, extensions).await;
        }

        let cached = self.lookup_cache(&origin);
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
        // for streaming bodies; then the response passes through as-is.
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
            // The server rejected a token we believed fresh (revoked
            // early). Drop it and its header on the clone, then re-acquire.
            self.store_cache(origin.clone(), TokenCache::Empty);
            retry_req
                .headers_mut()
                .remove(reqwest::header::AUTHORIZATION);
        }

        let Some(header) = self.acquire_and_cache(origin, &url, &challenges).await else {
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

    /// Router requiring `Bearer <accept>` on /channel/repodata.json,
    /// answering 401 + WWW-Authenticate otherwise. Counts every request.
    fn protected_router(accept: String, hits: Arc<AtomicUsize>) -> axum::Router {
        use axum::{http::StatusCode, response::IntoResponse, routing::get};
        axum::Router::new().route(
            "/channel/repodata.json",
            get(move |headers: axum::http::HeaderMap| {
                let hits = hits.clone();
                let expected = format!("Bearer {accept}");
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
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
        )
    }

    async fn spawn_protected_server(accept: &str, hits: Arc<AtomicUsize>) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = protected_router(accept.to_string(), hits);
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Url::parse(&format!("http://{addr}")).unwrap()
    }

    /// Like [`spawn_protected_server`], but the accepted token is derived
    /// from the server's own port (`token-<port>`), pairing with
    /// [`PortTokenFlow`] to make per-origin cache mix-ups observable.
    async fn spawn_port_token_server(hits: Arc<AtomicUsize>) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = protected_router(format!("token-{}", addr.port()), hits);
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
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));

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
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));

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
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));

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
        // `Negotiate YII=`: the trailing blob is not a key=value param
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
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));
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
    async fn existing_authorization_header_is_respected() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let flow = StaticFlow::new(Some("abc123"));
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));

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
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));

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
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));
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
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));
        let url = server_url.join("/channel/repodata.json").unwrap();

        // caller sees the server's 401, not the flow error
        assert_eq!(client.get(url.clone()).send().await.unwrap().status(), 401);
        assert_eq!(client.get(url).send().await.unwrap().status(), 401);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 1);
        // Disabled = pass-through: both requests still reach the server.
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn token68_with_double_padding_is_skipped() {
        let challenges = parse_challenges(&header_map(&["Negotiate dGVzdA=="]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].scheme, "Negotiate");
        assert!(challenges[0].params.is_empty());
    }

    /// Flow minting a token tied to the challenged URL's port; pairs with
    /// [`spawn_port_token_server`] to prove per-origin cache scoping.
    #[derive(Debug)]
    struct PortTokenFlow {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl AuthFlow for PortTokenFlow {
        async fn acquire_token(
            &self,
            url: &Url,
            _challenges: &[Challenge],
        ) -> Result<Option<BearerToken>, AuthFlowError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let port = url.port().expect("test URLs always carry a port");
            Ok(Some(BearerToken::new(format!("token-{port}"))))
        }
    }

    #[tokio::test]
    async fn tokens_are_cached_per_origin() {
        let hits_a = Arc::new(AtomicUsize::new(0));
        let hits_b = Arc::new(AtomicUsize::new(0));
        let url_a = spawn_port_token_server(hits_a.clone()).await;
        let url_b = spawn_port_token_server(hits_b.clone()).await;
        let flow = Arc::new(PortTokenFlow {
            calls: AtomicUsize::new(0),
        });
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));

        let a = url_a.join("/channel/repodata.json").unwrap();
        let b = url_b.join("/channel/repodata.json").unwrap();

        // Each origin mints its own token; the repeat request on origin A
        // must be served from A's cache entry (a single shared slot would
        // attach B's token and fail).
        assert_eq!(client.get(a.clone()).send().await.unwrap().status(), 200);
        assert_eq!(client.get(b).send().await.unwrap().status(), 200);
        assert_eq!(client.get(a).send().await.unwrap().status(), 200);

        assert_eq!(flow.calls.load(Ordering::SeqCst), 2); // once per origin
        assert_eq!(hits_a.load(Ordering::SeqCst), 3); // challenge + replay + cached
        assert_eq!(hits_b.load(Ordering::SeqCst), 2); // challenge + replay
    }

    #[tokio::test]
    async fn flows_are_consulted_in_order_until_one_yields() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let inapplicable = StaticFlow::new(None);
        let minting = StaticFlow::new(Some("abc123"));
        let client = client_with(AuthChallengeMiddleware::new(vec![
            inapplicable.clone(),
            minting.clone(),
        ]));

        let url = server_url.join("/channel/repodata.json").unwrap();
        assert_eq!(client.get(url.clone()).send().await.unwrap().status(), 200);
        // The token is cached: a second request consults no flow at all.
        assert_eq!(client.get(url).send().await.unwrap().status(), 200);

        assert_eq!(inapplicable.calls.load(Ordering::SeqCst), 1);
        assert_eq!(minting.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failing_flow_falls_through_to_next() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let failing = Arc::new(FailingFlow {
            calls: AtomicUsize::new(0),
        });
        let minting = StaticFlow::new(Some("abc123"));
        let client = client_with(AuthChallengeMiddleware::new(vec![
            failing.clone(),
            minting.clone(),
        ]));

        let url = server_url.join("/channel/repodata.json").unwrap();
        assert_eq!(client.get(url).send().await.unwrap().status(), 200);
        assert_eq!(failing.calls.load(Ordering::SeqCst), 1);
        assert_eq!(minting.calls.load(Ordering::SeqCst), 1);
    }

    /// Flow applicable to exactly one origin (matched by port).
    #[derive(Debug)]
    struct SinglePortFlow {
        port: u16,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl AuthFlow for SinglePortFlow {
        async fn acquire_token(
            &self,
            url: &Url,
            _challenges: &[Challenge],
        ) -> Result<Option<BearerToken>, AuthFlowError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if url.port() == Some(self.port) {
                Ok(Some(BearerToken::new(format!("token-{}", self.port))))
            } else {
                Ok(None)
            }
        }
    }

    #[tokio::test]
    async fn negative_cache_is_scoped_per_origin() {
        let hits_a = Arc::new(AtomicUsize::new(0));
        let hits_b = Arc::new(AtomicUsize::new(0));
        let url_a = spawn_port_token_server(hits_a.clone()).await;
        let url_b = spawn_port_token_server(hits_b.clone()).await;
        let flow = Arc::new(SinglePortFlow {
            port: url_b.port().unwrap(),
            calls: AtomicUsize::new(0),
        });
        let client = client_with(AuthChallengeMiddleware::new(vec![flow.clone()]));

        let a = url_a.join("/channel/repodata.json").unwrap();
        let b = url_b.join("/channel/repodata.json").unwrap();

        // Origin A: flow inapplicable -> negative-cached, asked exactly once.
        assert_eq!(client.get(a.clone()).send().await.unwrap().status(), 401);
        assert_eq!(client.get(a).send().await.unwrap().status(), 401);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 1);

        // Origin B is unaffected by A's negative entry.
        assert_eq!(client.get(b).send().await.unwrap().status(), 200);
        assert_eq!(flow.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn default_middleware_is_inert_for_untrusted_origins() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_url = spawn_protected_server("abc123", hits.clone()).await;
        let client = client_with(AuthChallengeMiddleware::default());

        let response = client
            .get(server_url.join("/channel/repodata.json").unwrap())
            .send()
            .await
            .unwrap();

        // A loopback http origin is outside every default flow's trust gate:
        // the caller sees the original 401 and there is no replay.
        assert_eq!(response.status(), 401);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
