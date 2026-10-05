//! Exchange of a CI provider's OIDC ID token for a bearer token.
//!
//! 1. Ask `ambient-id` for an OIDC ID token with the configured `audience`
//!    claim (`None` outside supported CI providers).
//! 2. Exchange it at the server (see [`ExchangeProtocol`]) for a
//!    short-lived bearer token.
//!
//! Used by `rattler auth login --workload-identity` and, through
//! [`crate::trusted_publishing`], for trusted publishing to prefix.dev.

use reqwest::{StatusCode, header::CONTENT_TYPE};
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::challenge_middleware::BearerToken;

/// Default path of the prefix.dev mint endpoint.
pub(crate) const DEFAULT_MINT_PATH: &str = "/api/oidc/mint_token";

/// Well-known path of OAuth 2.0 Authorization Server Metadata (RFC 8414 §3).
const AUTHORIZATION_SERVER_METADATA_PATH: &str = "/.well-known/oauth-authorization-server";

/// The RFC 8693 token exchange grant type.
const TOKEN_EXCHANGE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";

/// How the CI provider's OIDC ID token is exchanged for a bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeProtocol {
    /// OAuth 2.0 Token Exchange (RFC 8693) at `endpoint`.
    TokenExchange {
        /// The token endpoint, joined onto the server URL with [`Url::join`]:
        /// either an absolute URL or a path starting with `/`. RFC 8693
        /// doesn't define a path, so it must be configured.
        endpoint: String,
    },
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
    /// An RFC 8693 token exchange at `endpoint` (see
    /// [`ExchangeProtocol::TokenExchange`]).
    pub fn token_exchange(audience: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            audience: audience.into(),
            exchange: ExchangeProtocol::TokenExchange {
                endpoint: endpoint.into(),
            },
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

/// The fields of OAuth 2.0 Authorization Server Metadata (RFC 8414 §2) we use.
#[derive(Deserialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    token_endpoint: Option<String>,
    grant_types_supported: Option<Vec<String>>,
}

/// The metadata URL for `issuer`: the well-known path is inserted between
/// the host and the issuer's path, if any (RFC 8414 §3.1).
fn authorization_server_metadata_url(issuer: &Url) -> Url {
    let mut url = issuer.clone();
    let issuer_path = issuer.path().trim_end_matches('/');
    url.set_path(&format!(
        "{AUTHORIZATION_SERVER_METADATA_PATH}{issuer_path}"
    ));
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// Looks up the token endpoint of `issuer` in its OAuth 2.0 Authorization
/// Server Metadata (RFC 8414).
///
/// Returns `None` when the server publishes no metadata, or when the
/// metadata can't be used: it's for a different issuer (which RFC 8414 §3.3
/// forbids using), has no `token_endpoint`, or lists `grant_types_supported`
/// without the token exchange grant.
pub async fn discover_token_endpoint(
    client: &ClientWithMiddleware,
    issuer: &Url,
) -> Result<Option<Url>, OidcExchangeError> {
    let metadata_url = authorization_server_metadata_url(issuer);
    let response = client
        .get(metadata_url.clone())
        .send()
        .await
        .map_err(|err| OidcExchangeError::ReqwestMiddleware(metadata_url.clone(), err))?;
    if !response.status().is_success() {
        tracing::debug!(
            "No authorization server metadata at {metadata_url} (HTTP {})",
            response.status()
        );
        return Ok(None);
    }
    let body = response
        .bytes()
        .await
        .map_err(|err| OidcExchangeError::Reqwest(metadata_url.clone(), err))?;
    let Ok(metadata) = serde_json::from_slice::<AuthorizationServerMetadata>(&body) else {
        tracing::debug!("Invalid authorization server metadata at {metadata_url}");
        return Ok(None);
    };

    // RFC 8414 §3.3: the `issuer` in the metadata must be identical to the
    // issuer the metadata was requested for, or it must not be used.
    // Compared without a trailing slash, which `Url` always adds to an
    // empty path.
    if metadata.issuer.trim_end_matches('/') != issuer.as_str().trim_end_matches('/') {
        tracing::warn!(
            "Ignoring authorization server metadata at {metadata_url}: it is for issuer `{}`",
            metadata.issuer
        );
        return Ok(None);
    }
    // RFC 8414 §2: when `grant_types_supported` is omitted, the default
    // doesn't include token exchange. We still try the token endpoint then,
    // since many servers don't list every grant they support.
    if let Some(grant_types) = &metadata.grant_types_supported
        && !grant_types.iter().any(|g| g == TOKEN_EXCHANGE_GRANT_TYPE)
    {
        tracing::debug!("{metadata_url} doesn't list the token exchange grant type");
        return Ok(None);
    }
    Ok(metadata
        .token_endpoint
        .and_then(|endpoint| Url::parse(&endpoint).ok()))
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
        "Server returned error code {0} from the OIDC token exchange, is the server's OIDC trust configuration (issuer, audience) correct?\nResponse: {1}"
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
        ExchangeProtocol::TokenExchange { endpoint } => {
            token_exchange(oidc_token.reveal(), server_url, endpoint, client).await?
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
    endpoint: &str,
    client: &ClientWithMiddleware,
) -> Result<BearerToken, OidcExchangeError> {
    let exchange_url = server_url.join(endpoint)?;
    tracing::info!("Exchanging the OIDC token at {exchange_url}");

    // RFC 8693 §2.1: a form-encoded request. No client authentication: the
    // ID token identifies the caller, and the RFC leaves client
    // authentication to the server.
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", TOKEN_EXCHANGE_GRANT_TYPE)
        .append_pair(
            "subject_token_type",
            "urn:ietf:params:oauth:token-type:id_token",
        )
        .append_pair("subject_token", oidc_token)
        .finish();
    let request = client
        .post(exchange_url.clone())
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body);
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
    async fn token_exchange_reports_errors() {
        let (server_url, _) = token_server(
            "/oauth/token",
            (StatusCode::UNAUTHORIZED, r#"{"error":"invalid_target"}"#),
        )
        .await;
        let err = token_exchange(
            "fake.oidc.token",
            &server_url,
            "/oauth/token",
            &plain_client(),
        )
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
        let err = token_exchange(
            "fake.oidc.token",
            &server_url,
            "/oauth/token",
            &plain_client(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, OidcExchangeError::MissingAccessToken));
        assert!(!err.to_string().contains("must-not-leak"));
    }

    /// Serves `metadata` as the RFC 8414 metadata of the issuer at the
    /// server's root; `{issuer}` in it is replaced with the server's URL.
    async fn metadata_server(metadata: &'static str) -> Url {
        use axum::routing::get;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let metadata = metadata.replace("{issuer}", &format!("http://{addr}"));
        let router = axum::Router::new().route(
            "/.well-known/oauth-authorization-server",
            get(move || async move { metadata }),
        );
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Url::parse(&format!("http://{addr}")).unwrap()
    }

    #[tokio::test]
    async fn discovers_token_endpoint() {
        let issuer = metadata_server(
            r#"{"issuer":"{issuer}","token_endpoint":"https://auth.example.com/token","grant_types_supported":["urn:ietf:params:oauth:grant-type:token-exchange"]}"#,
        )
        .await;
        assert_eq!(
            discover_token_endpoint(&plain_client(), &issuer)
                .await
                .unwrap()
                .unwrap()
                .as_str(),
            "https://auth.example.com/token"
        );

        // `grant_types_supported` may be omitted.
        let issuer = metadata_server(
            r#"{"issuer":"{issuer}","token_endpoint":"https://auth.example.com/token"}"#,
        )
        .await;
        assert!(
            discover_token_endpoint(&plain_client(), &issuer)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn rejects_unusable_metadata() {
        for metadata in [
            // metadata for a different issuer (RFC 8414 §3.3)
            r#"{"issuer":"https://evil.example.com","token_endpoint":"https://evil.example.com/token"}"#,
            // token exchange not supported
            r#"{"issuer":"{issuer}","token_endpoint":"https://auth.example.com/token","grant_types_supported":["authorization_code"]}"#,
            // no token endpoint
            r#"{"issuer":"{issuer}"}"#,
        ] {
            let issuer = metadata_server(metadata).await;
            assert!(
                discover_token_endpoint(&plain_client(), &issuer)
                    .await
                    .unwrap()
                    .is_none(),
                "{metadata}"
            );
        }

        // no metadata at all
        let (server_url, _) = token_server("/oauth/token", (StatusCode::OK, "{}")).await;
        assert!(
            discover_token_endpoint(&plain_client(), &server_url)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn metadata_url_inserts_well_known_path() {
        let url = |issuer: &str| {
            authorization_server_metadata_url(&Url::parse(issuer).unwrap()).to_string()
        };
        assert_eq!(
            url("https://auth.example.com"),
            "https://auth.example.com/.well-known/oauth-authorization-server"
        );
        assert_eq!(
            url("https://example.com/tenant1/"),
            "https://example.com/.well-known/oauth-authorization-server/tenant1"
        );
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
