//! Trusted publishing (via OIDC).
//!
//! Provides [`TrustedPublishingFlow`] and [`PrefixAuthAmbientFlow`] for
//! [`crate::challenge_middleware`] and [`check_trusted_publishing`] for
//! uploads. The token exchange itself lives in [`crate::oidc_exchange`].

use std::sync::Arc;

use reqwest_middleware::ClientWithMiddleware;
use url::Url;

use crate::{
    challenge_middleware::{AuthFlow, AuthFlowError, BearerToken, Challenge},
    oidc_exchange::{
        DEFAULT_MINT_PATH, ExchangeProtocol, OidcExchangeError, OidcExchangeOptions, get_token,
        is_prefix_dev_host,
    },
};

/// Outcome of an optional trusted-publishing attempt.
pub enum TrustedPublishResult {
    /// We didn't check for trusted publishing (no CI provider detected).
    Skipped,
    /// We checked for trusted publishing and got a token.
    Configured(BearerToken),
    /// We checked for optional trusted publishing, but it didn't succeed.
    Ignored(OidcExchangeError),
}

/// Deprecated alias kept for backwards compatibility.
#[deprecated(note = "use `rattler_networking::BearerToken` instead")]
pub type TrustedPublishingToken = BearerToken;

/// If applicable, attempt to obtain a bearer token via trusted publishing.
///
/// Returns [`TrustedPublishResult::Skipped`] when `ambient-id` reports no
/// usable CI provider (the common case outside CI). Errors during the flow
/// are wrapped in [`TrustedPublishResult::Ignored`] so callers can fall back
/// to other auth sources without unwinding.
pub async fn check_trusted_publishing(
    client: &ClientWithMiddleware,
    server_url: &Url,
    options: &OidcExchangeOptions,
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

/// [`AuthFlow`] backed by trusted publishing (CI OIDC).
///
/// Responds only to `Bearer` challenges: asks `ambient-id` for an OIDC ID
/// token (`Ok(None)` outside supported CI providers) and exchanges it at
/// the challenged host's mint endpoint.
///
/// `client` is used only for the mint exchange; it must not itself layer
/// in [`crate::AuthChallengeMiddleware`] or the mint call will recurse.
///
/// # Security
///
/// `acquire_token` sends the CI provider's OIDC ID token (a live
/// credential) to `url`'s origin **without any origin validation of its
/// own**. Never register this flow directly in the unscoped
/// [`crate::AuthChallengeMiddleware`]; wrap it in an origin gate such as
/// [`PrefixAuthAmbientFlow`], or only drive it with URLs of a single
/// trusted host.
#[derive(Debug, Clone)]
pub struct TrustedPublishingFlow {
    options: OidcExchangeOptions,
    client: ClientWithMiddleware,
}

impl TrustedPublishingFlow {
    /// Create a flow with custom [`OidcExchangeOptions`]. A missing
    /// leading `/` on an [`ExchangeProtocol::PrefixMint`] path is normalized.
    pub fn new(mut options: OidcExchangeOptions, client: ClientWithMiddleware) -> Self {
        if let ExchangeProtocol::PrefixMint { path } = &mut options.exchange
            && !path.starts_with('/')
        {
            path.insert(0, '/');
        }
        Self { options, client }
    }

    /// Create a flow preconfigured for prefix.dev: audience `prefix.dev`,
    /// mint path `/api/oidc/mint_token`.
    pub fn for_prefix_dev(client: ClientWithMiddleware) -> Self {
        let options = OidcExchangeOptions {
            audience: "prefix.dev".to_string(),
            exchange: ExchangeProtocol::PrefixMint {
                path: DEFAULT_MINT_PATH.to_string(),
            },
        };
        Self::new(options, client)
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

/// Origin-gated [`AuthFlow`] for the prefix.dev family, safe to register
/// in an unscoped [`crate::AuthChallengeMiddleware`]; the default flow
/// behind [`crate::AuthChallengeMiddleware::default`].
///
/// Delegates to an inner flow (by default [`TrustedPublishingFlow`] with
/// [`TrustedPublishingFlow::for_prefix_dev`]) only for `https` URLs on
/// `prefix.dev` or a true subdomain. The gate keys on the request URL
/// alone; server-controlled challenge params such as `realm` cannot open
/// it. Outside CI the inner flow reports "not applicable".
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
                    OidcExchangeOptions {
                        audience: "prefix.dev".to_string(),
                        exchange: ExchangeProtocol::PrefixMint {
                            path: "api/x".to_string(),
                        },
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
