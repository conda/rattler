//! OAuth/OIDC authentication flows for the CLI.
//!
//! Supports authorization code grant with PKCE (primary) and device code
//! flow (fallback for headless environments).

use openidconnect::{
    AdditionalProviderMetadata, AuthorizationCode, ClientId, CsrfToken, IssuerUrl, Nonce,
    OAuth2TokenResponse, PkceCodeChallenge, ProviderMetadata, RedirectUrl, Scope,
    core::{
        CoreAuthDisplay, CoreClaimName, CoreClaimType, CoreClient, CoreClientAuthMethod,
        CoreGrantType, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm,
        CoreJweKeyManagementAlgorithm, CoreResponseMode, CoreResponseType,
        CoreSubjectIdentifierType,
    },
    AdditionalProviderMetadata, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    DeviceAuthorizationUrl, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge,
    ProviderMetadata, RedirectUrl, Scope, TokenResponse,
};
use rattler_networking::Authentication;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;
use tokio::net::TcpListener;
use url::Url;

/// Additional OIDC provider metadata fields not included in the standard
/// `ExtendedCoreProviderMetadata` type (RFC 7009 revocation, RFC 8628 device auth).
#[derive(Clone, Debug, Deserialize, Serialize)]
struct ExtendedProviderMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    revocation_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_authorization_endpoint: Option<String>,
}
impl AdditionalProviderMetadata for ExtendedProviderMetadata {}

/// Provider metadata type that includes revocation and device authorization
/// endpoints alongside the standard OIDC discovery fields.
type ExtendedCoreProviderMetadata = ProviderMetadata<
    ExtendedProviderMetadata,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

/// Configuration for an OAuth login flow.
pub struct OAuthConfig {
    /// The OIDC issuer URL.
    pub issuer_url: String,
    /// The OAuth client ID.
    pub client_id: String,
    /// Which flow to use.
    pub flow: OAuthFlow,
    /// Additional OAuth scopes to request.
    pub scopes: Vec<String>,
}

/// Which OAuth flow to attempt.
pub enum OAuthFlow {
    /// Try auth code first, fall back to device code on failure.
    Auto,
    /// Only use authorization code flow with PKCE.
    AuthCode,
    /// Only use device code flow.
    DeviceCode,
}

/// Extracted token values from an OAuth token response.
struct OAuthTokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<Duration>,
}

/// Errors that can occur during OAuth authentication.
#[derive(thiserror::Error, Debug)]
pub enum OAuthError {
    /// OIDC discovery failed.
    #[error("OIDC discovery failed: {0}")]
    Discovery(String),

    /// Authorization failed.
    #[error("Authorization failed: {0}")]
    Authorization(String),

    /// Token exchange failed.
    #[error("Token exchange failed: {0}")]
    TokenExchange(String),

    /// The callback from the identity provider was invalid.
    #[error("Invalid callback from identity provider")]
    InvalidCallback,

    /// The CSRF state token did not match.
    #[error("CSRF state token mismatch — possible CSRF attack")]
    CsrfMismatch,

    /// A network error occurred.
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A URL parsing error occurred.
    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    /// The provider does not support device authorization.
    #[error("Provider does not support device authorization flow")]
    DeviceCodeNotSupported,
}

/// Endpoints discovered from OIDC metadata.
struct DiscoveredEndpoints {
    provider_metadata: ExtendedCoreProviderMetadata,
    token_endpoint: String,
    revocation_endpoint: Option<String>,
    device_authorization_endpoint: Option<String>,
}

/// Perform an OAuth/OIDC login and return the resulting `Authentication::OAuth`.
pub async fn perform_oauth_login(config: OAuthConfig) -> Result<Authentication, OAuthError> {
    let http_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(OAuthError::Network)?;

    // 1. OIDC Discovery
    let endpoints = discover_endpoints(&http_client, &config.issuer_url).await?;

    // 2. Run the appropriate flow
    let tokens = match config.flow {
        OAuthFlow::AuthCode => {
            auth_code_flow(
                &endpoints.provider_metadata,
                &config.client_id,
                &config.scopes,
                &http_client,
            )
            .await?
        }
        OAuthFlow::DeviceCode => {
            device_code_flow(&endpoints, &config.client_id, &config.scopes, &http_client).await?
        }
        OAuthFlow::Auto => {
            match auth_code_flow(
                &endpoints.provider_metadata,
                &config.client_id,
                &config.scopes,
                &http_client,
            )
            .await
            {
                Ok(tokens) => tokens,
                Err(e) => {
                    eprintln!("Authorization code flow failed ({e}), trying device code flow...");
                    device_code_flow(&endpoints, &config.client_id, &config.scopes, &http_client)
                        .await?
                }
            }
        }
    };

    // 3. Build the Authentication::OAuth value
    let expires_at = tokens.expires_in.map(|d| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + d.as_secs() as i64
    });

    Ok(Authentication::OAuth {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        token_endpoint: endpoints.token_endpoint,
        revocation_endpoint: endpoints.revocation_endpoint,
        client_id: config.client_id,
    })
}

/// Perform OIDC discovery and extract all needed endpoints.
///
/// Uses our custom `ExtendedCoreProviderMetadata` type so that the
/// `revocation_endpoint` and `device_authorization_endpoint` fields are
/// deserialized from the discovery document in a single request.
async fn discover_endpoints(
    http_client: &reqwest::Client,
    issuer_url: &str,
) -> Result<DiscoveredEndpoints, OAuthError> {
    let oidc_issuer =
        IssuerUrl::new(issuer_url.to_string()).map_err(|e| OAuthError::Discovery(e.to_string()))?;

    let provider_metadata = ExtendedCoreProviderMetadata::discover_async(oidc_issuer, http_client)
        .await
        .map_err(|e| OAuthError::Discovery(e.to_string()))?;

    let token_endpoint = provider_metadata
        .token_endpoint()
        .map(|u| u.url().to_string())
        .unwrap_or_default();

    let extra = provider_metadata.additional_metadata();
    let revocation_endpoint = extra.revocation_endpoint.clone();
    let device_authorization_endpoint = extra.device_authorization_endpoint.clone();

    Ok(DiscoveredEndpoints {
        provider_metadata,
        token_endpoint,
        revocation_endpoint,
        device_authorization_endpoint,
    })
}

/// Authorization code flow with PKCE.
///
/// 1. Binds a local TCP listener for the redirect
/// 2. Opens the browser to the authorization URL
/// 3. Waits for the redirect callback
/// 4. Exchanges the authorization code for tokens
async fn auth_code_flow(
    provider_metadata: &ExtendedCoreProviderMetadata,
    client_id: &str,
    scopes: &[String],
    http_client: &reqwest::Client,
) -> Result<OAuthTokens, OAuthError> {
    // Bind to a random port on localhost
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = listener.local_addr()?;
    let redirect_url = format!("http://127.0.0.1:{}", local_addr.port());

    let client = CoreClient::from_provider_metadata(
        provider_metadata.clone(),
        ClientId::new(client_id.to_string()),
        None,
    )
    .set_redirect_uri(RedirectUrl::new(redirect_url).map_err(OAuthError::UrlParse)?);

    // Generate PKCE challenge
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    // Build authorization URL
    let mut auth_request = client.authorize_url(
        openidconnect::core::CoreAuthenticationFlow::AuthorizationCode,
        CsrfToken::new_random,
        Nonce::new_random,
    );
    for scope in scopes {
        auth_request = auth_request.add_scope(Scope::new(scope.clone()));
    }
    let (auth_url, csrf_token, _nonce) = auth_request.set_pkce_challenge(pkce_challenge).url();

    // Open browser
    let auth_url_str = auth_url.to_string();
    eprintln!("Opening browser for authentication...");
    eprintln!("If the browser does not open, visit: {auth_url_str}");

    if let Err(e) = open::that(&auth_url_str) {
        return Err(OAuthError::Authorization(format!(
            "Failed to open browser: {e}"
        )));
    }

    // Wait for the redirect callback
    let (code, state) = accept_redirect_callback(&listener).await?;

    // Verify CSRF state
    if state != *csrf_token.secret() {
        return Err(OAuthError::CsrfMismatch);
    }

    // Exchange code for tokens
    let token_response = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| OAuthError::TokenExchange(format!("token endpoint not configured: {e}")))?
        .set_pkce_verifier(pkce_verifier)
        .request_async(http_client)
        .await
        .map_err(|e| OAuthError::TokenExchange(e.to_string()))?;

    Ok(OAuthTokens {
        access_token: token_response.access_token().secret().clone(),
        refresh_token: token_response.refresh_token().map(|t| t.secret().clone()),
        expires_in: token_response.expires_in(),
    })
}

/// Accept the OAuth redirect callback on the local TCP listener.
///
/// Parses the `code` and `state` query parameters from the GET request,
/// sends a simple HTML response, and returns the extracted values.
async fn accept_redirect_callback(listener: &TcpListener) -> Result<(String, String), OAuthError> {
    let (stream, _) = listener.accept().await?;

    // Convert to std TcpStream for synchronous I/O (simpler than async line parsing)
    let std_stream = stream.into_std()?;
    std_stream.set_nonblocking(false)?;

    let mut reader = BufReader::new(std_stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Parse the GET request line: "GET /?code=...&state=... HTTP/1.1"
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or(OAuthError::InvalidCallback)?;

    let callback_url =
        Url::parse(&format!("http://localhost{path}")).map_err(|_| OAuthError::InvalidCallback)?;

    let code = callback_url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())
        .ok_or(OAuthError::InvalidCallback)?;

    let state = callback_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .ok_or(OAuthError::InvalidCallback)?;

    // Send a simple success response
    let response_body = "<html><body><h1>Authentication successful!</h1>\
        <p>You can close this window.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        response_body.len(),
        response_body
    );

    let mut writer = std_stream;
    writer.write_all(response.as_bytes())?;
    writer.flush()?;

    Ok((code, state))
}

/// Standard OAuth error response (RFC 6749 Section 5.2).
#[derive(Deserialize)]
struct OAuthErrorResponse {
    error: String,
    error_description: Option<String>,
}

impl std::fmt::Display for OAuthErrorResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(desc) = &self.error_description {
            write!(f, "{desc}")
        } else {
            write!(f, "{}", self.error)
        }
    }
}

/// Response from the device authorization endpoint (RFC 8628).
#[derive(Deserialize)]
struct DeviceAuthResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: Option<u64>,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_interval() -> u64 {
    5
}

/// Standard OAuth token response.
#[derive(Deserialize)]
#[serde(untagged)]
enum TokenResponse {
    Success {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
    },
    Error {
        error: String,
    },
}

/// Device code flow for headless environments (RFC 8628).
///
/// Implemented with raw HTTP requests to avoid openidconnect's complex
/// type-level endpoint state system.
async fn device_code_flow(
    endpoints: &DiscoveredEndpoints,
    client_id: &str,
    scopes: &[String],
    http_client: &reqwest::Client,
) -> Result<OAuthTokens, OAuthError> {
    let device_auth_endpoint = endpoints
        .device_authorization_endpoint
        .as_deref()
        .ok_or(OAuthError::DeviceCodeNotSupported)?;

    // Step 1: Request device authorization
    let scope_str = scopes.join(" ");
    let params = [("client_id", client_id), ("scope", &scope_str)];
    let resp = http_client
        .post(device_auth_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(OAuthError::Network)?;

    if !resp.status().is_success() {
        let status = resp.status();
        let msg = match resp.json::<OAuthErrorResponse>().await {
            Ok(err) => err.to_string(),
            Err(_) => format!("HTTP {status}"),
        };
        return Err(OAuthError::Authorization(msg));
    }

    let device_auth: DeviceAuthResponse = resp.json().await.map_err(|e| {
        OAuthError::Authorization(format!("Failed to parse device auth response: {e}"))
    })?;

    // Step 2: Display instructions
    eprintln!("\nTo authenticate, visit: {}", device_auth.verification_uri);
    eprintln!("And enter the code: {}\n", device_auth.user_code);

    if let Some(ref complete_uri) = device_auth.verification_uri_complete {
        eprintln!("Or visit: {complete_uri}");
    }

    // Step 3: Poll the token endpoint
    let poll_interval = Duration::from_secs(device_auth.interval);
    let timeout = Duration::from_secs(device_auth.expires_in.unwrap_or(300));
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        tokio::time::sleep(poll_interval).await;

        if tokio::time::Instant::now() >= deadline {
            return Err(OAuthError::TokenExchange(
                "Device code flow timed out".to_string(),
            ));
        }

        let params = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", &device_auth.device_code),
            ("client_id", client_id),
        ];

        let resp = http_client
            .post(&endpoints.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(OAuthError::Network)?;

        let body: TokenResponse = resp.json().await.map_err(|e| {
            OAuthError::TokenExchange(format!("Failed to parse token response: {e}"))
        })?;

        match body {
            TokenResponse::Error { error } => match error.as_str() {
                "authorization_pending" => continue,
                "slow_down" => {
                    tokio::time::sleep(poll_interval).await;
                    continue;
                }
                _ => {
                    return Err(OAuthError::TokenExchange(format!(
                        "Token endpoint error: {error}"
                    )));
                }
            },
            TokenResponse::Success {
                access_token,
                refresh_token,
                expires_in,
            } => {
                return Ok(OAuthTokens {
                    access_token,
                    refresh_token,
                    expires_in: expires_in.map(Duration::from_secs),
                });
            }
        }
    }
}

/// Revoke OAuth tokens at the provider's revocation endpoint.
///
/// Best-effort: logs warnings on failure but does not return errors.
pub async fn revoke_tokens(
    revocation_endpoint: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    client_id: &str,
) {
    let client = reqwest::Client::new();

    // Revoke refresh token first (higher priority)
    if let Some(refresh_token) = refresh_token {
        let params = [
            ("token", refresh_token),
            ("token_type_hint", "refresh_token"),
            ("client_id", client_id),
        ];
        match client.post(revocation_endpoint).form(&params).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!("Successfully revoked refresh token");
            }
            Ok(resp) => {
                tracing::warn!("Failed to revoke refresh token: HTTP {}", resp.status());
            }
            Err(e) => {
                tracing::warn!("Failed to revoke refresh token: {e}");
            }
        }
    }

    // Revoke access token
    let params = [
        ("token", access_token),
        ("token_type_hint", "access_token"),
        ("client_id", client_id),
    ];
    match client.post(revocation_endpoint).form(&params).send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::debug!("Successfully revoked access token");
        }
        Ok(resp) => {
            tracing::warn!("Failed to revoke access token: HTTP {}", resp.status());
        }
        Err(e) => {
            tracing::warn!("Failed to revoke access token: {e}");
        }
    }
}
