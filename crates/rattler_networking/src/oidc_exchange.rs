//! Exchange of a CI provider's OIDC ID token for a bearer token.
//!
//! 1. Ask `ambient-id` for an OIDC ID token with the configured `audience`
//!    claim (`None` outside supported CI providers).
//! 2. Exchange it at the server (see [`ExchangeProtocol`]) for a
//!    short-lived bearer token.
//!
//! Used by `rattler auth login --oidc` and, through
//! [`crate::trusted_publishing`], for trusted publishing to prefix.dev.

use reqwest::{StatusCode, header::CONTENT_TYPE};
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::challenge_middleware::BearerToken;

/// Default path of the prefix.dev mint endpoint.
pub(crate) const DEFAULT_MINT_PATH: &str = "/api/oidc/mint_token";

/// Path of the JFrog Access OIDC token exchange endpoint.
const JFROG_TOKEN_PATH: &str = "/access/api/v1/oidc/token";

/// How the CI provider's OIDC ID token is exchanged for a bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeProtocol {
    /// OAuth 2.0 Token Exchange (RFC 8693).
    TokenExchange(TokenExchange),
    /// The prefix.dev convention: `POST {"token": "<id token>"}` to `path`;
    /// the response body is the bearer token. This is prefix.dev's own API,
    /// not based on any standard.
    ///
    /// `path` is joined onto the server URL with [`Url::join`]; it must
    /// start with `/` or it would resolve relative to the URL's path.
    /// [`crate::trusted_publishing::TrustedPublishingFlow::new`] normalizes
    /// a missing leading slash.
    PrefixMint {
        /// Path on the server where the ID token is exchanged.
        path: String,
    },
}

/// Settings for an RFC 8693 token exchange.
///
/// The defaults follow the RFC; servers that deviate from it (like JFrog)
/// are expressed by changing [`encoding`](Self::encoding) and adding
/// [`extra_params`](Self::extra_params).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenExchange {
    /// The token endpoint, joined onto the server URL with [`Url::join`]:
    /// either an absolute URL or a path starting with `/`.
    pub endpoint: String,
    /// How the request body is encoded.
    pub encoding: RequestEncoding,
    /// Extension parameters sent in addition to the RFC 8693 ones. OAuth
    /// servers must ignore parameters they don't recognize (RFC 6749 §3.2),
    /// so these are allowed by the spec.
    pub extra_params: Vec<(String, String)>,
}

impl TokenExchange {
    /// A standard RFC 8693 exchange at `endpoint`.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            encoding: RequestEncoding::Form,
            extra_params: Vec::new(),
        }
    }
}

/// Encoding of the token exchange request body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RequestEncoding {
    /// `application/x-www-form-urlencoded`, as RFC 8693 §2.1 requires.
    #[default]
    Form,
    /// `application/json`. Deviates from RFC 8693 §2.1, but some servers
    /// (JFrog) only accept JSON.
    Json,
}

/// Which audience to request the ID token for and how to exchange it.
///
/// On GitLab CI the runner must populate the OIDC ID token under an env
/// var that `ambient-id` derives from [`audience`](Self::audience)
/// (uppercased, non-alphanumerics to `_`, suffixed `_ID_TOKEN`; audience
/// `prefix.dev` resolves to `PREFIX_DEV_ID_TOKEN`). Set it via the
/// `id_tokens` block in `.gitlab-ci.yml`.
#[derive(Debug, Clone)]
pub struct OidcExchangeOptions {
    /// The `aud` claim requested in the OIDC ID token. The server validates
    /// this against its trust configuration before issuing a token.
    pub audience: String,
    /// How the ID token is exchanged for a bearer token.
    pub exchange: ExchangeProtocol,
}

impl OidcExchangeOptions {
    /// A standard RFC 8693 exchange at `endpoint` (see
    /// [`TokenExchange::endpoint`]).
    pub fn token_exchange(audience: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            audience: audience.into(),
            exchange: ExchangeProtocol::TokenExchange(TokenExchange::new(endpoint)),
        }
    }

    /// JFrog Access' token exchange at `/access/api/v1/oidc/token`: RFC 8693
    /// with a JSON body, the `provider_name` of the OIDC integration to
    /// validate against and, on GitHub Actions, the job's context.
    ///
    /// Reads the GitHub Actions context from the environment when called.
    pub fn jfrog(audience: impl Into<String>, provider_name: impl Into<String>) -> Self {
        // JFrog extension: selects the OIDC integration to validate against.
        // A plain RFC 8693 server would pick its trust configuration from the
        // token's `iss` claim.
        let mut extra_params = vec![("provider_name".to_string(), provider_name.into())];
        extra_params.extend(jfrog_github_context());
        Self {
            audience: audience.into(),
            exchange: ExchangeProtocol::TokenExchange(TokenExchange {
                endpoint: JFROG_TOKEN_PATH.to_string(),
                // JFrog's endpoint only takes JSON.
                encoding: RequestEncoding::Json,
                extra_params,
            }),
        }
    }

    /// prefix.dev's mint endpoint on `server`. prefix.dev deployments
    /// (`prefix.dev` and `*.prefix.dev`) validate ID tokens against the
    /// shared audience `prefix.dev`; other servers following the prefix.dev
    /// convention use their host name, scoping each ID token to the server
    /// it is sent to. Falls back to `prefix.dev` when `server` has no host.
    ///
    /// Does not validate scheme or host; callers handling ambient CI
    /// credentials must enforce `https` and an allow-list themselves.
    pub fn prefix_dev(server: &Url) -> Self {
        let audience = match server.host_str() {
            Some(host) if !is_prefix_dev_host(host) => host.to_string(),
            _ => "prefix.dev".to_string(),
        };
        Self {
            audience,
            exchange: ExchangeProtocol::PrefixMint {
                path: DEFAULT_MINT_PATH.to_string(),
            },
        }
    }
}

/// Returns `true` for `prefix.dev` and any true subdomain (`*.prefix.dev`).
/// Lookalikes (`evil-prefix.dev`, `prefix.dev.evil.com`) and trailing-dot
/// hosts (`beta.prefix.dev.`, preserved as-is by [`Url`]) fail closed.
pub(crate) fn is_prefix_dev_host(host: &str) -> bool {
    host == "prefix.dev" || host.ends_with(".prefix.dev")
}

/// JFrog extension: GitHub Actions context sent along with the exchange.
/// The `gh_*` fields are used by JFrog for usage tracking, `repo`/
/// `revision`/`branch` are AppTrust context which JFrog policies can
/// evaluate. Empty outside GitHub Actions; unset variables are omitted
/// rather than sent empty.
fn jfrog_github_context() -> Vec<(String, String)> {
    if std::env::var("GITHUB_ACTIONS").ok().as_deref() != Some("true") {
        return Vec::new();
    }
    [
        ("gh_job_id", "GITHUB_JOB"),
        ("gh_run_id", "GITHUB_RUN_ID"),
        ("gh_repo", "GITHUB_REPOSITORY"),
        ("gh_revision", "GITHUB_SHA"),
        ("gh_branch", "GITHUB_REF_NAME"),
        ("repo", "GITHUB_REPOSITORY"),
        ("revision", "GITHUB_SHA"),
        ("branch", "GITHUB_REF_NAME"),
    ]
    .into_iter()
    .filter_map(|(param, var)| {
        let value = std::env::var(var).ok().filter(|v| !v.is_empty())?;
        Some((param.to_string(), value))
    })
    .collect()
}

/// Errors that can occur while exchanging an OIDC ID token.
#[derive(Debug, Error)]
pub enum OidcExchangeError {
    /// Failed to parse a URL.
    #[error(transparent)]
    Url(#[from] url::ParseError),
    /// HTTP request failed at the reqwest layer.
    #[error("Failed to fetch: `{0}`")]
    Reqwest(Url, #[source] reqwest::Error),
    /// HTTP request failed at the reqwest-middleware layer.
    #[error("Failed to fetch: `{0}`")]
    ReqwestMiddleware(Url, #[source] reqwest_middleware::Error),
    /// The prefix.dev mint endpoint returned an error.
    #[error(
        "Server returned error code {0} from the mint endpoint, is trusted publishing correctly configured?\nResponse: {1}"
    )]
    MintToken(StatusCode, String),
    /// The token exchange endpoint returned an error.
    #[error(
        "Server returned error code {0} from the OIDC token exchange, is the server's OIDC trust configuration (audience, provider) correct?\nResponse: {1}"
    )]
    TokenExchange(StatusCode, String),
    /// The token exchange succeeded but the response had no usable
    /// `access_token`.
    #[error("The OIDC token exchange response did not contain an `access_token`")]
    MissingAccessToken,
    /// Retrieving the OIDC ID token from the CI provider failed.
    #[error("Failed to retrieve an OIDC ID token from the CI provider")]
    OidcToken(#[from] ambient_id::Error),
}

/// Returns the short-lived token to use against `server_url`, or `None` when
/// `ambient-id` reports no usable CI provider.
///
/// Delegates OIDC ID-token retrieval to `ambient-id`; this function owns the
/// exchange with `server_url`.
pub async fn get_token(
    client: &ClientWithMiddleware,
    server_url: &Url,
    options: &OidcExchangeOptions,
) -> Result<Option<BearerToken>, OidcExchangeError> {
    let detector = ambient_id::Detector::new_with_client(client.clone());
    let Some(oidc_token) = detector.detect(&options.audience).await? else {
        return Ok(None);
    };

    let token = match &options.exchange {
        ExchangeProtocol::TokenExchange(exchange) => {
            token_exchange(oidc_token.reveal(), server_url, exchange, client).await?
        }
        ExchangeProtocol::PrefixMint { path } => {
            prefix_mint(oidc_token.reveal(), server_url, path, client).await?
        }
    };

    tracing::info!("Exchanged the OIDC token from the CI provider for a bearer token");

    Ok(Some(token))
}

/// The body sent to the prefix.dev mint endpoint.
#[derive(Serialize)]
struct MintTokenRequest<'a> {
    token: &'a str,
}

async fn prefix_mint(
    oidc_token: &str,
    server_url: &Url,
    mint_path: &str,
    client: &ClientWithMiddleware,
) -> Result<BearerToken, OidcExchangeError> {
    let mint_token_url = server_url.join(mint_path)?;
    tracing::info!("Querying the trusted publishing token from {mint_token_url}");

    let response = client
        .post(mint_token_url.clone())
        .json(&MintTokenRequest { token: oidc_token })
        .send()
        .await
        .map_err(|err| OidcExchangeError::ReqwestMiddleware(mint_token_url.clone(), err))?;

    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|err| OidcExchangeError::Reqwest(mint_token_url.clone(), err))?;

    if status.is_success() {
        Ok(BearerToken::new(String::from_utf8_lossy(&body).to_string()))
    } else {
        Err(OidcExchangeError::MintToken(
            status,
            String::from_utf8_lossy(&body).to_string(),
        ))
    }
}

/// The part of the RFC 8693 token exchange response we use.
///
/// RFC 8693 §2.2.1 also requires `issued_token_type` and `token_type`. We
/// only need `access_token`, so we don't fail if a server leaves them out.
#[derive(Deserialize)]
struct TokenExchangeResponse {
    access_token: Option<String>,
}

async fn token_exchange(
    oidc_token: &str,
    server_url: &Url,
    exchange: &TokenExchange,
    client: &ClientWithMiddleware,
) -> Result<BearerToken, OidcExchangeError> {
    let exchange_url = server_url.join(&exchange.endpoint)?;
    tracing::info!("Exchanging the OIDC token at {exchange_url}");

    // RFC 8693 §2.1. No client authentication: the ID token identifies the
    // caller, and the RFC leaves client authentication to the server.
    let mut params = vec![
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:token-exchange",
        ),
        (
            "subject_token_type",
            "urn:ietf:params:oauth:token-type:id_token",
        ),
        ("subject_token", oidc_token),
    ];
    params.extend(
        exchange
            .extra_params
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );

    let request = client.post(exchange_url.clone());
    let request = match exchange.encoding {
        RequestEncoding::Form => request
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(
                url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(&params)
                    .finish(),
            ),
        RequestEncoding::Json => request.json(
            &params
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.into()))
                .collect::<serde_json::Map<_, _>>(),
        ),
    };
    let response = request
        .send()
        .await
        .map_err(|err| OidcExchangeError::ReqwestMiddleware(exchange_url.clone(), err))?;

    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|err| OidcExchangeError::Reqwest(exchange_url.clone(), err))?;

    if !status.is_success() {
        return Err(OidcExchangeError::TokenExchange(
            status,
            String::from_utf8_lossy(&body).to_string(),
        ));
    }

    // Don't echo the body on a malformed success response: it may contain
    // the issued token.
    let token = serde_json::from_slice::<TokenExchangeResponse>(&body)
        .ok()
        .and_then(|response| response.access_token)
        .filter(|token| !token.is_empty())
        .ok_or(OidcExchangeError::MissingAccessToken)?;
    Ok(BearerToken::new(token))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        body::Bytes,
        http::{HeaderMap, StatusCode},
        routing::post,
    };

    use super::*;

    fn plain_client() -> ClientWithMiddleware {
        reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build()
    }

    /// A captured request: its `content-type` and raw body.
    type Captured = Arc<Mutex<Option<(String, String)>>>;

    /// Serves a token endpoint at `path` that captures the request and
    /// answers with `response`.
    async fn token_server(
        path: &'static str,
        response: (StatusCode, &'static str),
    ) -> (Url, Captured) {
        let seen: Captured = Arc::default();
        let seen_in_handler = seen.clone();
        let router = axum::Router::new().route(
            path,
            post(move |headers: HeaderMap, body: Bytes| {
                let seen = seen_in_handler.clone();
                async move {
                    let content_type = headers
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    *seen.lock().unwrap() =
                        Some((content_type, String::from_utf8_lossy(&body).to_string()));
                    response
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (Url::parse(&format!("http://{addr}")).unwrap(), seen)
    }

    /// Env vars that force `ambient-id`'s GitLab detector, which reads the
    /// ID token from an env var derived from the audience. (rattler's own CI
    /// runs on GitHub Actions, so `GITHUB_ACTIONS` must be explicitly unset.)
    fn gitlab_env(token_var: &'static str) -> [(&'static str, Option<&'static str>); 5] {
        [
            ("GITLAB_CI", Some("true")),
            (token_var, Some("fake.oidc.token")),
            ("GITHUB_ACTIONS", None),
            ("BUILDKITE", None),
            ("CIRCLECI", None),
        ]
    }

    #[tokio::test]
    async fn token_exchange_sends_form_encoded_rfc8693_request() {
        let (server_url, seen) = token_server(
            "/oauth/token",
            (
                StatusCode::OK,
                r#"{"access_token":"issued.token","issued_token_type":"urn:ietf:params:oauth:token-type:access_token","token_type":"Bearer"}"#,
            ),
        )
        .await;

        let token = temp_env::async_with_vars(
            gitlab_env("EXAMPLE_ID_TOKEN"),
            get_token(
                &plain_client(),
                &server_url,
                &OidcExchangeOptions::token_exchange("example", "/oauth/token"),
            ),
        )
        .await
        .unwrap();

        assert_eq!(token.expect("expected a token").secret(), "issued.token");
        let (content_type, body) = seen.lock().unwrap().take().unwrap();
        assert_eq!(content_type, "application/x-www-form-urlencoded");
        assert_eq!(
            body,
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange\
             &subject_token_type=urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aid_token\
             &subject_token=fake.oidc.token"
        );
    }

    #[tokio::test]
    async fn jfrog_exchange_sends_json_request() {
        let (server_url, seen) = token_server(
            "/access/api/v1/oidc/token",
            (
                StatusCode::OK,
                r#"{"access_token":"jfrog.access.token","token_type":"Bearer"}"#,
            ),
        )
        .await;

        let token = temp_env::async_with_vars(gitlab_env("JFROG_GITHUB_ID_TOKEN"), async {
            get_token(
                &plain_client(),
                &server_url,
                &OidcExchangeOptions::jfrog("jfrog-github", "github-oidc"),
            )
            .await
        })
        .await
        .unwrap();

        assert_eq!(
            token.expect("expected a token").secret(),
            "jfrog.access.token"
        );
        let (content_type, body) = seen.lock().unwrap().take().unwrap();
        assert_eq!(content_type, "application/json");
        // Outside GitHub Actions no GitHub context is sent.
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({
                "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
                "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
                "subject_token": "fake.oidc.token",
                "provider_name": "github-oidc",
            })
        );
    }

    #[tokio::test]
    async fn token_exchange_reports_errors() {
        let exchange = TokenExchange::new("/oauth/token");

        let (server_url, _) = token_server(
            "/oauth/token",
            (StatusCode::UNAUTHORIZED, r#"{"error":"invalid_target"}"#),
        )
        .await;
        let err = token_exchange("fake.oidc.token", &server_url, &exchange, &plain_client())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, OidcExchangeError::TokenExchange(status, body)
                if *status == 401 && body.contains("invalid_target")),
            "{err:?}"
        );

        let (server_url, _) = token_server(
            "/oauth/token",
            (StatusCode::OK, r#"{"secret":"must-not-leak"}"#),
        )
        .await;
        let err = token_exchange("fake.oidc.token", &server_url, &exchange, &plain_client())
            .await
            .unwrap_err();
        assert!(matches!(err, OidcExchangeError::MissingAccessToken));
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
                let params = jfrog_github_context();
                let params: Vec<_> = params
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                assert_eq!(
                    params,
                    [
                        ("gh_job_id", "build"),
                        ("gh_run_id", "42"),
                        ("gh_repo", "org/repo"),
                        ("gh_revision", "abc123"),
                        ("repo", "org/repo"),
                        ("revision", "abc123"),
                    ]
                );
            },
        );
        temp_env::with_vars([("GITHUB_ACTIONS", None::<&str>)], || {
            assert!(jfrog_github_context().is_empty());
        });
    }

    #[test]
    fn prefix_dev_audience() {
        let audience =
            |url: &str| OidcExchangeOptions::prefix_dev(&Url::parse(url).unwrap()).audience;

        // prefix.dev deployments share the audience "prefix.dev"
        assert_eq!(audience("https://prefix.dev"), "prefix.dev");
        assert_eq!(
            audience("https://beta.prefix.dev/some-channel"),
            "prefix.dev"
        );
        // other hosts use their normalized host name
        assert_eq!(
            audience("https://conda.example.com/channel"),
            "conda.example.com"
        );
        assert_eq!(
            audience("https://Conda.EXAMPLE.com:443/channel"),
            "conda.example.com"
        );
        // lookalike hosts are not part of the family
        assert_eq!(
            audience("https://evil-prefix.dev/channel"),
            "evil-prefix.dev"
        );
        // no host falls back to prefix.dev
        assert_eq!(audience("data:text/plain,hello"), "prefix.dev");

        assert_eq!(
            OidcExchangeOptions::prefix_dev(&Url::parse("https://prefix.dev").unwrap()).exchange,
            ExchangeProtocol::PrefixMint {
                path: "/api/oidc/mint_token".to_string()
            }
        );
    }
}
