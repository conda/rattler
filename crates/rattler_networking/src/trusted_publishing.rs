//! Trusted publishing (via OIDC).
//!
//! This module owns the OIDC exchange with the server's mint endpoint and
//! provides [`TrustedPublishingFlow`], an [`AuthFlow`] implementation that
//! plugs into [`crate::challenge_middleware`]. Challenge-reactive HTTP
//! authentication (reacting to `WWW-Authenticate` responses) lives in
//! [`crate::challenge_middleware`].
//!
//! The flow:
//! 1. Ask `ambient-id` for an OIDC ID token with the configured `audience`
//!    claim. It owns CI-provider detection and returns `None` when no
//!    supported provider is present.
//! 2. Exchange that ID token at the server's mint endpoint for a short-lived
//!    bearer token usable against the server (read or write, depending on
//!    server policy).

use std::sync::Arc;

use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use serde::Serialize;
use thiserror::Error;
use url::Url;

use crate::challenge_middleware::{AuthFlow, AuthFlowError, BearerToken, Challenge};

/// Default path of the prefix.dev-convention mint endpoint.
const DEFAULT_MINT_PATH: &str = "/api/oidc/mint_token";

/// Knobs for the trusted-publishing flow. Use
/// [`for_prefix_dev`](Self::for_prefix_dev) for the prefix.dev defaults, or
/// construct directly to point at a different server.
///
/// On GitLab CI, the OIDC ID token must be populated by the runner under an
/// env var whose name is derived from [`audience`](Self::audience) by
/// `ambient-id` (uppercasing the audience and replacing non-alphanumeric
/// characters with `_`, then suffixing `_ID_TOKEN`). For audience
/// `prefix.dev`, that resolves to `PREFIX_DEV_ID_TOKEN` — set this via the
/// `id_tokens` block in `.gitlab-ci.yml`.
#[derive(Debug, Clone)]
pub struct TrustedPublishingOptions {
    /// The `aud` claim requested in the OIDC ID token. The server validates
    /// this against the trusted-publisher configuration before minting a
    /// token.
    pub audience: String,
    /// Path on the server where the ID token is exchanged for a bearer token.
    ///
    /// This path is joined onto arbitrary URLs of the challenged server using
    /// [`Url::join`]. It should start with `/` so that it replaces the full
    /// URL path; a relative path would resolve against the challenged URL's
    /// path and could target an unintended endpoint.
    /// [`TrustedPublishingFlow::new`] normalizes a missing leading slash.
    pub mint_path: String,
}

impl TrustedPublishingOptions {
    /// Options preconfigured for prefix.dev: audience `prefix.dev`, mint path
    /// `/api/oidc/mint_token`.
    pub fn for_prefix_dev() -> Self {
        Self {
            audience: "prefix.dev".to_string(),
            mint_path: DEFAULT_MINT_PATH.to_string(),
        }
    }

    /// Options for any trusted-publishing server following the prefix.dev
    /// convention: the OIDC audience is the server's host name and tokens
    /// are minted at `/api/oidc/mint_token`.
    ///
    /// Returns `None` when `server` has no host (e.g. `data:` URLs).
    ///
    /// Deriving the audience from the host scopes each OIDC ID token to the
    /// server it is sent to: a token minted with `aud = <host>` is only
    /// redeemable at that host.
    ///
    /// This constructor does not validate the scheme or host. Callers
    /// handling ambient CI credentials must ensure `server` uses `https`
    /// and apply their own host allow-list before attaching this flow.
    /// The audience is the URL-normalized host: lowercased, IDN hosts in
    /// punycode, and without any port. See the struct-level docs for how
    /// the audience determines the GitLab CI env-var name.
    pub fn for_host(server: &Url) -> Option<Self> {
        Some(Self {
            audience: server.host_str()?.to_string(),
            mint_path: DEFAULT_MINT_PATH.to_string(),
        })
    }

    /// Options for `server` following the deployed prefix.dev ecosystem
    /// convention: every prefix.dev deployment (`prefix.dev` and
    /// `*.prefix.dev`, e.g. `beta.prefix.dev`) validates GitHub OIDC tokens
    /// against the shared audience `prefix.dev`, while tokens are minted at
    /// the deployment's own host. Hosts outside that family fall back to
    /// [`Self::for_host`] (host-derived audience).
    ///
    /// Returns `None` when `server` has no host. The caveats on
    /// [`Self::for_host`] (scheme validation, allow-listing) apply here too.
    pub fn for_server(server: &Url) -> Option<Self> {
        let host = server.host_str()?;
        if host == "prefix.dev" || host.ends_with(".prefix.dev") {
            Some(Self::for_prefix_dev())
        } else {
            Self::for_host(server)
        }
    }
}

/// Outcome of an optional trusted-publishing attempt.
pub enum TrustedPublishResult {
    /// We didn't check for trusted publishing (no CI provider detected).
    Skipped,
    /// We checked for trusted publishing and got a token.
    Configured(BearerToken),
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
}

/// Deprecated alias kept for backwards compatibility.
#[deprecated(note = "use `rattler_networking::BearerToken` instead")]
pub type TrustedPublishingToken = BearerToken;

/// The body sent to the server's mint endpoint.
#[derive(Serialize)]
struct MintTokenRequest {
    token: String,
}

/// If applicable, attempt to obtain a bearer token via trusted publishing.
///
/// Returns [`TrustedPublishResult::Skipped`] when `ambient-id` reports no
/// usable CI provider (the common case outside CI). Errors during the flow
/// are wrapped in [`TrustedPublishResult::Ignored`] so callers can fall back
/// to other auth sources without unwinding.
pub async fn check_trusted_publishing(
    client: &ClientWithMiddleware,
    server_url: &Url,
    options: &TrustedPublishingOptions,
) -> TrustedPublishResult {
    match get_token(client, server_url, options).await {
        Ok(Some(token)) => TrustedPublishResult::Configured(token),
        Ok(None) => TrustedPublishResult::Skipped,
        Err(err) => {
            tracing::debug!("Could not obtain trusted publishing credentials, skipping: {err}");
            TrustedPublishResult::Ignored(err)
        }
    }
}

/// Returns the short-lived token to use against `server_url`, or `None` when
/// `ambient-id` reports no usable CI provider.
///
/// Delegates OIDC ID-token retrieval to `ambient-id`; this function owns the
/// mint exchange with `server_url`.
pub async fn get_token(
    client: &ClientWithMiddleware,
    server_url: &Url,
    options: &TrustedPublishingOptions,
) -> Result<Option<BearerToken>, TrustedPublishingError> {
    let detector = ambient_id::Detector::new_with_client(client.clone());
    let Some(oidc_token) = detector.detect(&options.audience).await? else {
        return Ok(None);
    };

    let publish_token = get_publish_token(&oidc_token, server_url, client, options).await?;

    tracing::info!("Received OIDC token from CI provider, using trusted publishing");

    Ok(Some(publish_token))
}

async fn get_publish_token(
    oidc_token: &ambient_id::IdToken,
    server_url: &Url,
    client: &ClientWithMiddleware,
    options: &TrustedPublishingOptions,
) -> Result<BearerToken, TrustedPublishingError> {
    let mint_token_url = server_url.join(&options.mint_path)?;
    tracing::info!("Querying the trusted publishing token from {mint_token_url}");
    let mint_token_payload = MintTokenRequest {
        token: oidc_token.reveal().to_string(),
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
        Ok(BearerToken::new(String::from_utf8_lossy(&body).to_string()))
    } else {
        Err(TrustedPublishingError::MintToken(
            status,
            String::from_utf8_lossy(&body).to_string(),
        ))
    }
}

/// [`AuthFlow`] implementation backed by trusted publishing (CI OIDC).
///
/// Responds only to `Bearer` challenges. On a challenge it asks `ambient-id`
/// for an OIDC ID token (returns `Ok(None)` outside supported CI providers)
/// and exchanges it at the challenged host's mint endpoint
/// ([`TrustedPublishingOptions::mint_path`]).
///
/// `client` is used only for the mint exchange; it must not itself layer in
/// [`crate::AuthChallengeMiddleware`] or the mint call will recurse.
///
/// # Security
///
/// `acquire_token` sends the CI provider's OIDC ID token — a live
/// credential — to `url`'s origin (joined with
/// [`TrustedPublishingOptions::mint_path`]) **without any origin
/// validation of its own**. [`crate::AuthChallengeMiddleware`] invokes
/// flows for every challenged URL, so never register this flow there
/// directly: wrap it in an origin gate such as [`PrefixAuthAmbientFlow`],
/// or only drive it with URLs for a single host you trust.
#[derive(Debug, Clone)]
pub struct TrustedPublishingFlow {
    options: TrustedPublishingOptions,
    client: ClientWithMiddleware,
}

impl TrustedPublishingFlow {
    /// Create a flow with custom [`TrustedPublishingOptions`]. A missing
    /// leading `/` on [`TrustedPublishingOptions::mint_path`] is normalized.
    pub fn new(mut options: TrustedPublishingOptions, client: ClientWithMiddleware) -> Self {
        if !options.mint_path.starts_with('/') {
            options.mint_path.insert(0, '/');
        }
        Self { options, client }
    }

    /// Create a flow preconfigured for prefix.dev.
    pub fn for_prefix_dev(client: ClientWithMiddleware) -> Self {
        Self::new(TrustedPublishingOptions::for_prefix_dev(), client)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl AuthFlow for TrustedPublishingFlow {
    async fn acquire_token(
        &self,
        url: &Url,
        challenges: &[Challenge],
    ) -> Result<Option<BearerToken>, AuthFlowError> {
        if !challenges
            .iter()
            .any(|challenge| challenge.scheme.eq_ignore_ascii_case("bearer"))
        {
            return Ok(None);
        }
        get_token(&self.client, url, &self.options)
            .await
            .map_err(AuthFlowError::new)
    }
}

/// Returns `true` for `prefix.dev` and any true subdomain (`*.prefix.dev`).
/// Lookalikes (`evil-prefix.dev`, `prefix.dev.evil.com`) and trailing-dot
/// hosts (`beta.prefix.dev.`, preserved as-is by [`Url`]) fail closed.
fn is_prefix_dev_host(host: &str) -> bool {
    host == "prefix.dev" || host.ends_with(".prefix.dev")
}

/// Origin-gated [`AuthFlow`] for the prefix.dev family, safe to register in
/// an unscoped [`crate::AuthChallengeMiddleware`]: it forwards the ambient
/// CI identity only to origins it trusts.
///
/// On a challenge it delegates to an inner flow (by default a
/// [`TrustedPublishingFlow`] with [`TrustedPublishingOptions::for_prefix_dev`]
/// options) if and only if the challenged URL is `https` and its host is
/// `prefix.dev` or a true subdomain. The gate keys on the request URL alone —
/// server-controlled challenge parameters such as `realm` cannot open it.
/// Outside CI the inner flow finds no ambient OIDC identity and the flow
/// reports "not applicable".
///
/// This is the default flow behind [`crate::AuthChallengeMiddleware::default`].
#[derive(Debug, Clone)]
pub struct PrefixAuthAmbientFlow {
    inner: Arc<dyn AuthFlow>,
}

impl PrefixAuthAmbientFlow {
    /// Create the flow with `client` used for the mint exchange. The client
    /// must not itself layer in [`crate::AuthChallengeMiddleware`] or the
    /// mint call will recurse.
    pub fn new(client: ClientWithMiddleware) -> Self {
        Self::wrapping(Arc::new(TrustedPublishingFlow::for_prefix_dev(client)))
    }

    /// Apply the prefix.dev origin gate to an arbitrary `inner` flow:
    /// `inner` is only consulted for `https` URLs on the prefix.dev family.
    pub fn wrapping(inner: Arc<dyn AuthFlow>) -> Self {
        Self { inner }
    }
}

impl Default for PrefixAuthAmbientFlow {
    /// The flow with a plain (middleware-free) HTTP client for the mint
    /// exchange.
    fn default() -> Self {
        Self::new(reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl AuthFlow for PrefixAuthAmbientFlow {
    async fn acquire_token(
        &self,
        url: &Url,
        challenges: &[Challenge],
    ) -> Result<Option<BearerToken>, AuthFlowError> {
        if url.scheme() != "https" || !url.host_str().is_some_and(is_prefix_dev_host) {
            return Ok(None);
        }
        self.inner.acquire_token(url, challenges).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::challenge_middleware::{AuthFlow, Challenge};

    fn bearer_challenge() -> Vec<Challenge> {
        vec![Challenge {
            scheme: "Bearer".to_string(),
            params: HashMap::new(),
        }]
    }

    fn plain_client() -> reqwest_middleware::ClientWithMiddleware {
        reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build()
    }

    #[tokio::test]
    async fn flow_ignores_non_bearer_challenges() {
        let flow = TrustedPublishingFlow::for_prefix_dev(plain_client());
        let challenges = vec![Challenge {
            scheme: "Basic".to_string(),
            params: HashMap::new(),
        }];
        let result = flow
            .acquire_token(
                &Url::parse("https://prefix.dev/channel/repodata.json").unwrap(),
                &challenges,
            )
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn flow_mints_token_via_gitlab_env() {
        use axum::{Json, routing::post};

        // Mint endpoint: verifies it receives the CI-provided OIDC token and
        // returns the minted bearer token as the raw response body.
        let router = axum::Router::new().route(
            "/api/oidc/mint_token",
            post(|Json(body): Json<serde_json::Value>| async move {
                assert_eq!(body["token"], "fake.oidc.token");
                "pfx-jwt.minted"
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let server_url = Url::parse(&format!("http://{addr}")).unwrap();

        // Force the GitLab detector: GITLAB_CI on, every other provider off.
        // (rattler's own CI runs on GitHub Actions, so GITHUB_ACTIONS must be
        // explicitly unset.)
        let token = temp_env::async_with_vars(
            [
                ("GITLAB_CI", Some("true")),
                ("PREFIX_DEV_ID_TOKEN", Some("fake.oidc.token")),
                ("GITHUB_ACTIONS", None),
                ("BUILDKITE", None),
                ("CIRCLECI", None),
            ],
            async {
                let flow = TrustedPublishingFlow::for_prefix_dev(plain_client());
                flow.acquire_token(
                    &server_url.join("/channel/repodata.json").unwrap(),
                    &bearer_challenge(),
                )
                .await
                .unwrap()
            },
        )
        .await;

        assert_eq!(
            token.expect("expected a minted token").secret(),
            "pfx-jwt.minted"
        );
    }

    #[tokio::test]
    async fn mint_path_without_leading_slash_is_normalized() {
        use axum::routing::post;

        // Mint endpoint at the absolute path /api/x. Without normalization,
        // a relative mint_path of "api/x" would resolve against the
        // challenged URL's path (/channel/api/x) and miss this route.
        let router = axum::Router::new().route("/api/x", post(|| async { "pfx-jwt.minted" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let server_url = Url::parse(&format!("http://{addr}")).unwrap();

        let token = temp_env::async_with_vars(
            [
                ("GITLAB_CI", Some("true")),
                ("PREFIX_DEV_ID_TOKEN", Some("fake.oidc.token")),
                ("GITHUB_ACTIONS", None),
                ("BUILDKITE", None),
                ("CIRCLECI", None),
            ],
            async {
                let flow = TrustedPublishingFlow::new(
                    TrustedPublishingOptions {
                        audience: "prefix.dev".to_string(),
                        mint_path: "api/x".to_string(),
                    },
                    plain_client(),
                );
                flow.acquire_token(
                    &server_url.join("/channel/repodata.json").unwrap(),
                    &bearer_challenge(),
                )
                .await
                .unwrap()
            },
        )
        .await;

        assert_eq!(
            token.expect("expected a minted token").secret(),
            "pfx-jwt.minted"
        );
    }

    #[tokio::test]
    async fn middleware_with_trusted_publishing_flow_end_to_end() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        use axum::{
            Json,
            http::StatusCode,
            response::IntoResponse,
            routing::{get, post},
        };

        use crate::AuthChallengeMiddleware;

        // One server hosting both the protected resource and the mint
        // endpoint, like a real prefix.dev instance.
        let mints = Arc::new(AtomicUsize::new(0));
        let mints_in_handler = mints.clone();
        let router = axum::Router::new()
            .route(
                "/channel/repodata.json",
                get(|headers: axum::http::HeaderMap| async move {
                    match headers.get("authorization").and_then(|v| v.to_str().ok()) {
                        Some("Bearer pfx-jwt.minted") => (StatusCode::OK, "ok").into_response(),
                        _ => (
                            StatusCode::UNAUTHORIZED,
                            [("www-authenticate", r#"Bearer realm="test""#)],
                            "unauthorized",
                        )
                            .into_response(),
                    }
                }),
            )
            .route(
                "/api/oidc/mint_token",
                post(move |Json(body): Json<serde_json::Value>| {
                    let mints = mints_in_handler.clone();
                    async move {
                        assert_eq!(body["token"], "fake.oidc.token");
                        mints.fetch_add(1, Ordering::SeqCst);
                        "pfx-jwt.minted"
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let server_url = Url::parse(&format!("http://{addr}")).unwrap();

        temp_env::async_with_vars(
            [
                ("GITLAB_CI", Some("true")),
                ("PREFIX_DEV_ID_TOKEN", Some("fake.oidc.token")),
                ("GITHUB_ACTIONS", None),
                ("BUILDKITE", None),
                ("CIRCLECI", None),
            ],
            async {
                // The mint client must not itself carry the challenge
                // middleware (it would recurse), so it stays plain.
                let flow = TrustedPublishingFlow::for_prefix_dev(plain_client());
                let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
                    .with_arc(std::sync::Arc::new(AuthChallengeMiddleware::new(vec![
                        std::sync::Arc::new(flow),
                    ])))
                    .build();
                let url = server_url.join("/channel/repodata.json").unwrap();

                // First request: challenge -> OIDC detect -> mint -> replay.
                assert_eq!(client.get(url.clone()).send().await.unwrap().status(), 200);
                // Second request: cached token, no second mint.
                assert_eq!(client.get(url).send().await.unwrap().status(), 200);
            },
        )
        .await;

        assert_eq!(mints.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn for_prefix_dev_matches_prefix_dev() {
        let opts = TrustedPublishingOptions::for_prefix_dev();
        assert_eq!(opts.audience, "prefix.dev");
        assert_eq!(opts.mint_path, "/api/oidc/mint_token");
    }

    #[test]
    fn for_host_derives_audience_from_host() {
        let options = TrustedPublishingOptions::for_host(
            &Url::parse("https://beta.prefix.dev/some-channel/noarch/repodata.json").unwrap(),
        )
        .unwrap();
        assert_eq!(options.audience, "beta.prefix.dev");
        assert_eq!(options.mint_path, "/api/oidc/mint_token");

        let prod =
            TrustedPublishingOptions::for_host(&Url::parse("https://prefix.dev").unwrap()).unwrap();
        assert_eq!(
            prod.audience,
            TrustedPublishingOptions::for_prefix_dev().audience
        );
        assert_eq!(
            prod.mint_path,
            TrustedPublishingOptions::for_prefix_dev().mint_path
        );
    }

    #[test]
    fn for_host_returns_none_without_host() {
        // data: URLs have no host component
        let url = Url::parse("data:text/plain,hello").unwrap();
        assert!(TrustedPublishingOptions::for_host(&url).is_none());
    }

    #[test]
    fn for_host_normalizes_case_and_drops_default_port() {
        let options = TrustedPublishingOptions::for_host(
            &Url::parse("https://Beta.PREFIX.dev:443/some-channel").unwrap(),
        )
        .unwrap();
        assert_eq!(options.audience, "beta.prefix.dev");
    }

    #[test]
    fn for_server_uses_shared_audience_for_prefix_dev_family() {
        // prefix.dev deployments share the audience "prefix.dev"
        let beta = TrustedPublishingOptions::for_server(
            &Url::parse("https://beta.prefix.dev/some-channel").unwrap(),
        )
        .unwrap();
        assert_eq!(beta.audience, "prefix.dev");

        let prod = TrustedPublishingOptions::for_server(&Url::parse("https://prefix.dev").unwrap())
            .unwrap();
        assert_eq!(prod.audience, "prefix.dev");

        // hosts outside the family keep the host-derived audience
        let other = TrustedPublishingOptions::for_server(
            &Url::parse("https://conda.example.com/channel").unwrap(),
        )
        .unwrap();
        assert_eq!(other.audience, "conda.example.com");

        // ...and lookalike hosts are not part of the family
        let evil = TrustedPublishingOptions::for_server(
            &Url::parse("https://evil-prefix.dev/channel").unwrap(),
        )
        .unwrap();
        assert_eq!(evil.audience, "evil-prefix.dev");
    }

    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use crate::challenge_middleware::{AuthFlowError, BearerToken};

    /// Inner flow recording invocations; stands in for the trusted-publishing
    /// delegate so the gate can be observed without any network traffic.
    #[derive(Debug)]
    struct SpyFlow {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl AuthFlow for SpyFlow {
        async fn acquire_token(
            &self,
            _url: &Url,
            _challenges: &[Challenge],
        ) -> Result<Option<BearerToken>, AuthFlowError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(BearerToken::new("spy-token".to_string())))
        }
    }

    #[tokio::test]
    async fn ambient_flow_delegates_for_prefix_dev_family_hosts() {
        let spy = Arc::new(SpyFlow {
            calls: AtomicUsize::new(0),
        });
        let flow = PrefixAuthAmbientFlow::wrapping(spy.clone());
        for host in ["prefix.dev", "beta.prefix.dev", "staging.beta.prefix.dev"] {
            let url = Url::parse(&format!("https://{host}/channel/repodata.json")).unwrap();
            let token = flow.acquire_token(&url, &bearer_challenge()).await.unwrap();
            assert!(token.is_some(), "{host} should pass the trust gate");
        }
        assert_eq!(spy.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn ambient_flow_never_delegates_for_untrusted_origins() {
        let spy = Arc::new(SpyFlow {
            calls: AtomicUsize::new(0),
        });
        let flow = PrefixAuthAmbientFlow::wrapping(spy.clone());
        // The gate must key on the request URL alone: a server-controlled
        // realm claiming "prefix.dev" must not open it.
        let challenges = vec![Challenge {
            scheme: "Bearer".to_string(),
            params: HashMap::from([("realm".to_string(), "prefix.dev".to_string())]),
        }];
        for url in [
            "https://evil-prefix.dev/channel/repodata.json",
            "https://prefix.dev.evil.com/channel/repodata.json",
            "https://conda.anaconda.org/conda-forge/noarch/repodata.json",
            "http://prefix.dev/channel/repodata.json", // https only
            "https://beta.prefix.dev./channel/repodata.json", // trailing dot fails closed
        ] {
            let url = Url::parse(url).unwrap();
            let token = flow.acquire_token(&url, &challenges).await.unwrap();
            assert!(token.is_none(), "{url} must be rejected by the trust gate");
        }
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
    }
}
