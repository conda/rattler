//! OAuth/OIDC authentication flows for the CLI.
//!
//! Supports authorization code grant with PKCE (primary) and device code
//! flow (fallback for headless environments).

use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    time::Duration,
};

use openidconnect::{
    core::{
        CoreAuthDisplay, CoreClaimName, CoreClaimType, CoreClient, CoreClientAuthMethod,
        CoreDeviceAuthorizationResponse, CoreGrantType, CoreIdTokenClaims, CoreJsonWebKey,
        CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreResponseMode,
        CoreResponseType, CoreSubjectIdentifierType,
    },
    AdditionalProviderMetadata, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    DeviceAuthorizationUrl, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge,
    ProviderMetadata, RedirectUrl, Scope, TokenResponse,
};
use rattler_networking::Authentication;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use url::Url;

/// Additional OIDC provider metadata fields not included in the standard
/// `ExtendedCoreProviderMetadata` type (RFC 7009 revocation, RFC 8628 device
/// auth).
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
    /// The OAuth client secret (for confidential clients).
    pub client_secret: Option<String>,
    /// Which flow to use.
    pub flow: OAuthFlow,
    /// Additional OAuth scopes to request.
    pub scopes: HashSet<String>,
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
    authenticated_as: Option<String>,
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
    #[error(transparent)]
    Network(#[from] reqwest::Error),

    /// An I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A URL parsing error occurred.
    #[error(transparent)]
    UrlParse(#[from] url::ParseError),

    /// Failed to open the browser for authentication.
    #[error("Failed to open browser: {0}")]
    BrowserOpen(String),

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

/// Parsed values from the OAuth redirect callback.
struct CallbackResult {
    code: String,
    state: String,
    /// The TCP stream to send the browser response on after the token exchange.
    stream: std::net::TcpStream,
}

/// Perform an OAuth/OIDC login and return the resulting
/// `Authentication::OAuth`.
pub async fn perform_oauth_login(config: OAuthConfig) -> Result<Authentication, OAuthError> {
    let http_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(OAuthError::Network)?;

    // 1. OIDC Discovery
    let endpoints = discover_endpoints(&http_client, &config.issuer_url).await?;

    let client_secret = config.client_secret.as_deref();

    // 2. Run the appropriate flow
    let tokens = match config.flow {
        OAuthFlow::AuthCode => {
            auth_code_flow(
                &endpoints,
                &config.client_id,
                client_secret,
                &config.scopes,
                &http_client,
            )
            .await?
        }
        OAuthFlow::DeviceCode => {
            device_code_flow(
                &endpoints,
                &config.client_id,
                client_secret,
                &config.scopes,
                &http_client,
            )
            .await?
        }
        OAuthFlow::Auto => {
            match auth_code_flow(
                &endpoints,
                &config.client_id,
                client_secret,
                &config.scopes,
                &http_client,
            )
            .await
            {
                Ok(tokens) => tokens,
                Err(OAuthError::BrowserOpen(e)) => {
                    eprintln!("Failed to open browser ({e}), falling back to device code flow...");
                    device_code_flow(
                        &endpoints,
                        &config.client_id,
                        client_secret,
                        &config.scopes,
                        &http_client,
                    )
                    .await?
                }
                Err(e) => return Err(e),
            }
        }
    };

    // 3. Display authenticated identity
    match &tokens.authenticated_as {
        Some(identity) => eprintln!("Authenticated as: {identity}"),
        None => eprintln!("Authentication successful."),
    }

    // 4. Build the Authentication::OAuth value
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
        .ok_or_else(|| {
            OAuthError::Discovery("provider metadata does not include a token endpoint".into())
        })?;

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
    endpoints: &DiscoveredEndpoints,
    client_id: &str,
    client_secret: Option<&str>,
    scopes: &HashSet<String>,
    http_client: &reqwest::Client,
) -> Result<OAuthTokens, OAuthError> {
    // Bind to a random port on localhost
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = listener.local_addr()?;
    let redirect_url = format!("http://127.0.0.1:{}", local_addr.port());

    let mut client = CoreClient::from_provider_metadata(
        endpoints.provider_metadata.clone(),
        ClientId::new(client_id.to_string()),
        client_secret.map(|s| ClientSecret::new(s.to_string())),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_url).map_err(OAuthError::UrlParse)?);

    // Public clients (no secret) must send client_id in the request body
    if client_secret.is_none() {
        client = client.set_auth_type(openidconnect::AuthType::RequestBody);
    }

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
    let (auth_url, csrf_token, nonce) = auth_request.set_pkce_challenge(pkce_challenge).url();

    // Open browser
    let auth_url_str = auth_url.to_string();
    if let Err(e) = open::that(&auth_url_str) {
        return Err(OAuthError::BrowserOpen(e.to_string()));
    }

    // Print that we are opening the browser
    eprintln!("Opening browser at:\n\n{auth_url_str}");

    // Wait for the redirect callback (with timeout)
    eprintln!("Waiting for authentication in browser...");
    let callback = tokio::time::timeout(
        Duration::from_secs(300),
        accept_redirect_callback(&listener),
    )
    .await
    .map_err(|_timeout| {
        OAuthError::Authorization(
            "timed out waiting for browser authentication — please try again".into(),
        )
    })??;

    // Verify CSRF state
    if callback.state != *csrf_token.secret() {
        send_callback_response(&callback.stream, false, "CSRF state mismatch");
        return Err(OAuthError::CsrfMismatch);
    }

    // Exchange code for tokens
    let token_response = match client
        .exchange_code(AuthorizationCode::new(callback.code))
        .map_err(|e| OAuthError::TokenExchange(format!("token endpoint not configured: {e}")))?
        .set_pkce_verifier(pkce_verifier)
        .request_async(http_client)
        .await
    {
        Ok(response) => {
            send_callback_response(&callback.stream, true, "");
            response
        }
        Err(e) => {
            let msg = e.to_string();
            send_callback_response(&callback.stream, false, &msg);
            return Err(OAuthError::TokenExchange(msg));
        }
    };

    let authenticated_as = token_response.id_token().and_then(|id_token| {
        match id_token.claims(&client.id_token_verifier(), &nonce) {
            Ok(claims) => Some(display_name_from_claims(claims)),
            Err(e) => {
                tracing::debug!("ID token verification failed: {e}");
                None
            }
        }
    });

    Ok(OAuthTokens {
        access_token: token_response.access_token().secret().clone(),
        refresh_token: token_response.refresh_token().map(|t| t.secret().clone()),
        expires_in: token_response.expires_in(),
        authenticated_as,
    })
}

/// Accept the OAuth redirect callback on the local TCP listener.
///
/// Parses the `code` and `state` query parameters from the GET request
/// and returns them along with the stream. The caller is responsible for
/// sending the browser response via [`send_callback_response`] after
/// the token exchange completes.
async fn accept_redirect_callback(listener: &TcpListener) -> Result<CallbackResult, OAuthError> {
    let (stream, _) = listener.accept().await?;

    // Convert to std TcpStream for synchronous I/O (simpler than async line
    // parsing)
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

    let callback_url = Url::parse(&format!("http://localhost{path}"))
        .map_err(|_err| OAuthError::InvalidCallback)?;

    // Check for an error response from the identity provider (RFC 6749 Section
    // 4.1.2.1)
    if let Some(error) = callback_url
        .query_pairs()
        .find(|(k, _)| k == "error")
        .map(|(_, v)| v.to_string())
    {
        let description = callback_url
            .query_pairs()
            .find(|(k, _)| k == "error_description")
            .map(|(_, v)| v.to_string());

        let msg = description.unwrap_or(error);

        send_callback_response(&std_stream, false, &msg);
        return Err(OAuthError::Authorization(msg));
    }

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

    Ok(CallbackResult {
        code,
        state,
        stream: std_stream,
    })
}

/// Escape a string for safe interpolation into HTML.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Send an HTML response to the browser on the callback stream.
fn send_callback_response(stream: &std::net::TcpStream, success: bool, detail: &str) {
    let response_body = if success {
        "<html><body><h1>Authentication successful!</h1>\
            <p>You can close this window and return to the terminal.</p></body></html>"
            .to_string()
    } else {
        let escaped = html_escape(detail);
        format!(
            "<html><body><h1>Authentication failed</h1><p>{escaped}</p>\
                <p>Please return to the terminal and try again.</p></body></html>"
        )
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {response_body}",
        response_body.len(),
    );
    let mut writer = stream;
    let _ = writer
        .write_all(response.as_bytes())
        .and_then(|_| writer.flush());
}

/// Device code flow for headless environments (RFC 8628).
///
/// Uses the openidconnect crate's high-level API, which automatically
/// includes the `openid` scope and handles polling with backoff.
async fn device_code_flow(
    endpoints: &DiscoveredEndpoints,
    client_id: &str,
    client_secret: Option<&str>,
    scopes: &HashSet<String>,
    http_client: &reqwest::Client,
) -> Result<OAuthTokens, OAuthError> {
    let device_auth_url = endpoints
        .device_authorization_endpoint
        .as_deref()
        .ok_or(OAuthError::DeviceCodeNotSupported)?;

    let device_auth_url = DeviceAuthorizationUrl::new(device_auth_url.to_string())
        .map_err(|e| OAuthError::Authorization(format!("Invalid device authorization URL: {e}")))?;

    let mut client = CoreClient::from_provider_metadata(
        endpoints.provider_metadata.clone(),
        ClientId::new(client_id.to_string()),
        client_secret.map(|s| ClientSecret::new(s.to_string())),
    )
    .set_device_authorization_url(device_auth_url);

    // Public clients (no secret) must send client_id in the request body
    if client_secret.is_none() {
        client = client.set_auth_type(openidconnect::AuthType::RequestBody);
    }

    // Step 1: Request device authorization
    let mut device_request = client.exchange_device_code();
    for scope in scopes {
        device_request = device_request.add_scope(Scope::new(scope.clone()));
    }

    let details: CoreDeviceAuthorizationResponse = device_request
        .request_async(http_client)
        .await
        .map_err(|e| OAuthError::Authorization(format!("Device authorization failed: {e}")))?;

    // Step 2: Display instructions
    if let Some(complete_uri) = details.verification_uri_complete() {
        eprintln!(
            "\nOpen this link to authenticate directly:\n\n  {}\n",
            complete_uri.secret()
        );
        eprintln!(
            "Or visit {} and enter code:  {}\n",
            details.verification_uri().as_str(),
            details.user_code().secret()
        );
    } else {
        eprintln!(
            "\nTo authenticate, visit:\n\n  {}\n\nAnd enter code:  {}\n",
            details.verification_uri().as_str(),
            details.user_code().secret()
        );
    }

    // Step 3: Poll the token endpoint
    eprintln!("Waiting for authorization...");
    let token_response = client
        .exchange_device_access_token(&details)
        .map_err(|e| OAuthError::TokenExchange(format!("token endpoint not configured: {e}")))?
        .request_async(http_client, tokio::time::sleep, None)
        .await
        .map_err(|e| OAuthError::TokenExchange(e.to_string()))?;

    // Device flow has no nonce (RFC 8628), so skip nonce verification
    let authenticated_as = token_response.id_token().and_then(|id_token| {
        match id_token.claims(&client.id_token_verifier(), |_: Option<&Nonce>| Ok(())) {
            Ok(claims) => Some(display_name_from_claims(claims)),
            Err(e) => {
                tracing::debug!("ID token verification failed: {e}");
                None
            }
        }
    });

    Ok(OAuthTokens {
        access_token: token_response.access_token().secret().clone(),
        refresh_token: token_response.refresh_token().map(|t| t.secret().clone()),
        expires_in: token_response.expires_in(),
        authenticated_as,
    })
}

/// Extract a display name from ID token claims.
///
/// Prefers email > `preferred_username` > name > subject.
fn display_name_from_claims(claims: &CoreIdTokenClaims) -> String {
    if let Some(email) = claims.email() {
        return email.to_string();
    }
    if let Some(username) = claims.preferred_username() {
        return username.to_string();
    }
    if let Some(name) = claims.name() {
        if let Some(n) = name.get(None) {
            return n.to_string();
        }
    }
    claims.subject().to_string()
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
