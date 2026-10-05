//! Trusted publishing (via OIDC).
//!
//! Owns the exchange of the CI provider's OIDC ID token for a bearer token
//! (prefix.dev's mint endpoint or JFrog's token exchange) and provides
//! [`TrustedPublishingFlow`] and [`PrefixAuthAmbientFlow`] for
//! [`crate::challenge_middleware`].
//!
//! The flow:
//! 1. Ask `ambient-id` for an OIDC ID token with the configured `audience`
//!    claim (`None` outside supported CI providers).
//! 2. Exchange it at the server (see [`ExchangeProtocol`]) for a
//!    short-lived bearer token.

use std::sync::Arc;

use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::challenge_middleware::{AuthFlow, AuthFlowError, BearerToken, Challenge};

/// Default path of the prefix.dev-convention mint endpoint.
const DEFAULT_MINT_PATH: &str = "/api/oidc/mint_token";

/// Path of the JFrog Access OIDC token exchange endpoint.
const JFROG_TOKEN_PATH: &str = "/access/api/v1/oidc/token";

/// How the CI provider's OIDC ID token is exchanged for a bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeProtocol {
    /// The prefix.dev convention: `POST {"token": "<id token>"}` to `path`;
    /// the response body is the bearer token.
    ///
    /// `path` is joined onto the server URL with [`Url::join`]; it must
    /// start with `/` or it would resolve relative to the URL's path.
    /// [`TrustedPublishingFlow::new`] normalizes a missing leading slash.
    PrefixMint {
        /// Path on the server where the ID token is exchanged.
        path: String,
    },
    /// JFrog Access' OIDC token exchange at `/access/api/v1/oidc/token`,
    /// modeled on OAuth 2.0 Token Exchange (RFC 8693).
    Jfrog {
        /// Name of the OIDC integration configured in JFrog, which decides
        /// how the ID token is validated and mapped to a JFrog identity.
        provider_name: String,
    },
}

/// Knobs for the trusted-publishing flow. Use
/// [`for_prefix_dev`](Self::for_prefix_dev) for the prefix.dev defaults.
///
/// On GitLab CI the runner must populate the OIDC ID token under an env
/// var that `ambient-id` derives from [`audience`](Self::audience)
/// (uppercased, non-alphanumerics to `_`, suffixed `_ID_TOKEN`; audience
/// `prefix.dev` resolves to `PREFIX_DEV_ID_TOKEN`). Set it via the
/// `id_tokens` block in `.gitlab-ci.yml`.
#[derive(Debug, Clone)]
pub struct TrustedPublishingOptions {
    /// The `aud` claim requested in the OIDC ID token. The server validates
    /// this against its trusted-publisher configuration before minting a
    /// token.
    pub audience: String,
    /// How the ID token is exchanged for a bearer token.
    pub exchange: ExchangeProtocol,
}

impl TrustedPublishingOptions {
    /// Options preconfigured for prefix.dev: audience `prefix.dev`, mint path
    /// `/api/oidc/mint_token`.
    pub fn for_prefix_dev() -> Self {
        Self {
            audience: "prefix.dev".to_string(),
            exchange: ExchangeProtocol::PrefixMint {
                path: DEFAULT_MINT_PATH.to_string(),
            },
        }
    }

    /// Options for a server following the prefix.dev convention: the OIDC
    /// audience is the server's host name (scoping each ID token to the
    /// server it is sent to) and tokens are minted at
    /// `/api/oidc/mint_token`.
    ///
    /// Returns `None` when `server` has no host. Does not validate scheme
    /// or host; callers handling ambient CI credentials must enforce
    /// `https` and an allow-list themselves. The audience is the
    /// URL-normalized host: lowercased, punycode, no port.
    pub fn for_host(server: &Url) -> Option<Self> {
        Some(Self {
            audience: server.host_str()?.to_string(),
            exchange: ExchangeProtocol::PrefixMint {
                path: DEFAULT_MINT_PATH.to_string(),
            },
        })
    }

    /// Like [`Self::for_host`], except prefix.dev deployments
    /// (`prefix.dev` and `*.prefix.dev`) get the shared audience
    /// `prefix.dev`, which is what they validate GitHub OIDC tokens
    /// against; tokens are still minted at the deployment's own host.
    ///
    /// Returns `None` when `server` has no host. The [`Self::for_host`]
    /// caveats apply.
    pub fn for_server(server: &Url) -> Option<Self> {
        let host = server.host_str()?;
        if host == "prefix.dev" || host.ends_with(".prefix.dev") {
            Some(Self::for_prefix_dev())
        } else {
            Self::for_host(server)
        }
    }

    /// Options for a JFrog instance: the ID token is requested with
    /// `audience` and exchanged against the OIDC integration named
    /// `provider_name`. Both must match the integration configured in JFrog.
    pub fn for_jfrog(audience: impl Into<String>, provider_name: impl Into<String>) -> Self {
        Self {
            audience: audience.into(),
            exchange: ExchangeProtocol::Jfrog {
                provider_name: provider_name.into(),
            },
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
    /// The JFrog token exchange endpoint returned an error.
    #[error(
        "Server returned error code {0} from the OIDC token exchange, are the OIDC integration and audience correctly configured?\nResponse: {1}"
    )]
    TokenExchange(StatusCode, String),
    /// The JFrog token exchange succeeded but the response had no usable
    /// `access_token`.
    #[error("The OIDC token exchange response did not contain an `access_token`")]
    MissingAccessToken,
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
    match &options.exchange {
        ExchangeProtocol::PrefixMint { path } => {
            prefix_mint(oidc_token.reveal(), server_url, path, client).await
        }
        ExchangeProtocol::Jfrog { provider_name } => {
            jfrog_token_exchange(oidc_token.reveal(), server_url, provider_name, client).await
        }
    }
}

async fn prefix_mint(
    oidc_token: &str,
    server_url: &Url,
    mint_path: &str,
    client: &ClientWithMiddleware,
) -> Result<BearerToken, TrustedPublishingError> {
    let mint_token_url = server_url.join(mint_path)?;
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
        Ok(BearerToken::new(String::from_utf8_lossy(&body).to_string()))
    } else {
        Err(TrustedPublishingError::MintToken(
            status,
            String::from_utf8_lossy(&body).to_string(),
        ))
    }
}

/// The body sent to JFrog's OIDC token exchange endpoint.
///
/// The standard RFC 8693 parameters plus JFrog extensions. OAuth servers
/// must ignore parameters they don't recognize (RFC 6749 §3.2), so the
/// extensions are allowed by the spec.
#[derive(Serialize)]
struct JfrogTokenRequest<'a> {
    grant_type: &'static str,
    subject_token_type: &'static str,
    subject_token: &'a str,
    /// JFrog extension: selects the OIDC integration to validate against.
    /// A plain RFC 8693 server would pick its trust configuration from the
    /// token's `iss` claim or the standard `resource`/`audience` parameters.
    provider_name: &'a str,
    #[serde(flatten)]
    github: Option<JfrogGithubContext>,
}

/// JFrog extension: GitHub Actions context sent along with the exchange.
/// Only populated on GitHub Actions; unset variables are omitted rather than
/// sent empty.
#[derive(Serialize, Default, Debug, PartialEq, Eq)]
struct JfrogGithubContext {
    // Used by JFrog for usage tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    gh_job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gh_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gh_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gh_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gh_branch: Option<String>,
    // AppTrust context, which JFrog policies can evaluate.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
}

impl JfrogGithubContext {
    /// Reads the context from the GitHub Actions environment, or `None`
    /// outside GitHub Actions.
    fn from_env() -> Option<Self> {
        if std::env::var("GITHUB_ACTIONS").ok().as_deref() != Some("true") {
            return None;
        }
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Some(Self {
            gh_job_id: var("GITHUB_JOB"),
            gh_run_id: var("GITHUB_RUN_ID"),
            gh_repo: var("GITHUB_REPOSITORY"),
            gh_revision: var("GITHUB_SHA"),
            gh_branch: var("GITHUB_REF_NAME"),
            repo: var("GITHUB_REPOSITORY"),
            revision: var("GITHUB_SHA"),
            branch: var("GITHUB_REF_NAME"),
        })
    }
}

/// The part of JFrog's token exchange response we use.
///
/// RFC 8693 §2.2.1 also requires `issued_token_type` and `token_type`. We
/// only need `access_token`, so we don't fail if JFrog leaves them out.
#[derive(Deserialize)]
struct JfrogTokenResponse {
    access_token: Option<String>,
}

async fn jfrog_token_exchange(
    oidc_token: &str,
    server_url: &Url,
    provider_name: &str,
    client: &ClientWithMiddleware,
) -> Result<BearerToken, TrustedPublishingError> {
    let exchange_url = server_url.join(JFROG_TOKEN_PATH)?;
    tracing::info!("Exchanging the OIDC token at {exchange_url}");
    let payload = JfrogTokenRequest {
        grant_type: "urn:ietf:params:oauth:grant-type:token-exchange",
        subject_token_type: "urn:ietf:params:oauth:token-type:id_token",
        subject_token: oidc_token,
        provider_name,
        github: JfrogGithubContext::from_env(),
    };

    // Deviates from RFC 8693 §2.1, which requires an
    // `application/x-www-form-urlencoded` body like every OAuth token
    // request: JFrog's endpoint takes JSON, as its own clients send it.
    let response = client
        .post(exchange_url.clone())
        .json(&payload)
        .send()
        .await
        .map_err(|err| TrustedPublishingError::ReqwestMiddleware(exchange_url.clone(), err))?;

    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|err| TrustedPublishingError::Reqwest(exchange_url.clone(), err))?;

    if !status.is_success() {
        return Err(TrustedPublishingError::TokenExchange(
            status,
            String::from_utf8_lossy(&body).to_string(),
        ));
    }

    // Don't echo the body on a malformed success response: it may contain
    // the issued token.
    let token = serde_json::from_slice::<JfrogTokenResponse>(&body)
        .ok()
        .and_then(|response| response.access_token)
        .filter(|token| !token.is_empty())
        .ok_or(TrustedPublishingError::MissingAccessToken)?;
    Ok(BearerToken::new(token))
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
    options: TrustedPublishingOptions,
    client: ClientWithMiddleware,
}

impl TrustedPublishingFlow {
    /// Create a flow with custom [`TrustedPublishingOptions`]. A missing
    /// leading `/` on an [`ExchangeProtocol::PrefixMint`] path is normalized.
    pub fn new(mut options: TrustedPublishingOptions, client: ClientWithMiddleware) -> Self {
        if let ExchangeProtocol::PrefixMint { path } = &mut options.exchange
            && !path.starts_with('/')
        {
            path.insert(0, '/');
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

/// Origin-gated [`AuthFlow`] for the prefix.dev family, safe to register
/// in an unscoped [`crate::AuthChallengeMiddleware`]; the default flow
/// behind [`crate::AuthChallengeMiddleware::default`].
///
/// Delegates to an inner flow (by default [`TrustedPublishingFlow`] with
/// [`TrustedPublishingOptions::for_prefix_dev`]) only for `https` URLs on
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
                    TrustedPublishingOptions {
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

    /// Serves a JFrog-style token exchange endpoint that records the
    /// request body and answers with `response`.
    async fn jfrog_server(
        response: (axum::http::StatusCode, &'static str),
    ) -> (Url, Arc<std::sync::Mutex<Option<serde_json::Value>>>) {
        use axum::{Json, routing::post};

        let seen = Arc::new(std::sync::Mutex::new(None));
        let seen_in_handler = seen.clone();
        let router = axum::Router::new().route(
            "/access/api/v1/oidc/token",
            post(move |Json(body): Json<serde_json::Value>| {
                let seen = seen_in_handler.clone();
                async move {
                    *seen.lock().unwrap() = Some(body);
                    response
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (Url::parse(&format!("http://{addr}")).unwrap(), seen)
    }

    #[tokio::test]
    async fn jfrog_exchange_sends_token_exchange_request() {
        let (server_url, seen) = jfrog_server((
            axum::http::StatusCode::OK,
            r#"{"access_token":"jfrog.access.token","token_type":"Bearer"}"#,
        ))
        .await;

        let token = temp_env::async_with_vars(
            [
                ("GITLAB_CI", Some("true")),
                ("JFROG_GITHUB_ID_TOKEN", Some("fake.oidc.token")),
                ("GITHUB_ACTIONS", None),
                ("BUILDKITE", None),
                ("CIRCLECI", None),
            ],
            get_token(
                &plain_client(),
                &server_url,
                &TrustedPublishingOptions::for_jfrog("jfrog-github", "github-oidc"),
            ),
        )
        .await
        .unwrap();

        assert_eq!(
            token.expect("expected a token").secret(),
            "jfrog.access.token"
        );
        // Outside GitHub Actions no GitHub context is sent.
        assert_eq!(
            seen.lock().unwrap().take().unwrap(),
            serde_json::json!({
                "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
                "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
                "subject_token": "fake.oidc.token",
                "provider_name": "github-oidc",
            })
        );
    }

    #[tokio::test]
    async fn jfrog_exchange_reports_errors() {
        let (server_url, _) = jfrog_server((
            axum::http::StatusCode::UNAUTHORIZED,
            r#"{"errors":[{"message":"bad audience"}]}"#,
        ))
        .await;
        let err = jfrog_token_exchange("fake.oidc.token", &server_url, "p", &plain_client())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, TrustedPublishingError::TokenExchange(status, body)
                if *status == 401 && body.contains("bad audience")),
            "{err:?}"
        );

        let (server_url, _) =
            jfrog_server((axum::http::StatusCode::OK, r#"{"secret":"must-not-leak"}"#)).await;
        let err = jfrog_token_exchange("fake.oidc.token", &server_url, "p", &plain_client())
            .await
            .unwrap_err();
        assert!(matches!(err, TrustedPublishingError::MissingAccessToken));
        assert!(!err.to_string().contains("must-not-leak"));
    }

    #[test]
    fn jfrog_github_context_from_env() {
        temp_env::with_vars(
            [
                ("GITHUB_ACTIONS", Some("true")),
                ("GITHUB_JOB", Some("build")),
                ("GITHUB_RUN_ID", Some("42")),
                ("GITHUB_REPOSITORY", Some("org/repo")),
                ("GITHUB_SHA", Some("abc123")),
                ("GITHUB_REF_NAME", Some("")),
            ],
            || {
                let context = JfrogGithubContext::from_env().unwrap();
                assert_eq!(
                    serde_json::to_value(&context).unwrap(),
                    serde_json::json!({
                        "gh_job_id": "build",
                        "gh_run_id": "42",
                        "gh_repo": "org/repo",
                        "gh_revision": "abc123",
                        "repo": "org/repo",
                        "revision": "abc123",
                    })
                );
            },
        );
        temp_env::with_vars([("GITHUB_ACTIONS", None::<&str>)], || {
            assert_eq!(JfrogGithubContext::from_env(), None);
        });
    }

    #[test]
    fn for_prefix_dev_matches_prefix_dev() {
        let opts = TrustedPublishingOptions::for_prefix_dev();
        assert_eq!(opts.audience, "prefix.dev");
        assert_eq!(
            opts.exchange,
            ExchangeProtocol::PrefixMint {
                path: "/api/oidc/mint_token".to_string()
            }
        );
    }

    #[test]
    fn for_host_derives_audience_from_host() {
        let options = TrustedPublishingOptions::for_host(
            &Url::parse("https://beta.prefix.dev/some-channel/noarch/repodata.json").unwrap(),
        )
        .unwrap();
        assert_eq!(options.audience, "beta.prefix.dev");
        assert_eq!(
            options.exchange,
            TrustedPublishingOptions::for_prefix_dev().exchange
        );

        let prod =
            TrustedPublishingOptions::for_host(&Url::parse("https://prefix.dev").unwrap()).unwrap();
        assert_eq!(
            prod.audience,
            TrustedPublishingOptions::for_prefix_dev().audience
        );
        assert_eq!(
            prod.exchange,
            TrustedPublishingOptions::for_prefix_dev().exchange
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
