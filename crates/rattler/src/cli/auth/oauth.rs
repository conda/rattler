//! OAuth/OIDC authentication flows for the CLI.
//!
//! Supports authorization code grant with PKCE (primary) and device code
//! flow (fallback for headless environments).

use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    time::Duration,
};

use oauth2_reqwest::ReqwestClient;
use openidconnect::{
    AdditionalProviderMetadata, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    DeviceAuthorizationUrl, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge,
    ProviderMetadata, RedirectUrl, Scope, TokenResponse,
    core::{
        CoreAuthDisplay, CoreClaimName, CoreClaimType, CoreClient, CoreClientAuthMethod,
        CoreDeviceAuthorizationResponse, CoreGrantType, CoreIdTokenClaims, CoreJsonWebKey,
        CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreResponseMode,
        CoreResponseType, CoreSubjectIdentifierType,
    },
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

use super::DEFAULT_USER_AGENT;

/// Generic OIDC scopes used when no host-specific defaults apply.
pub const DEFAULT_OAUTH_SCOPES: &[&str] = &["openid", "profile", "offline_access"];

/// Renderer for the HTML page shown in the browser after the OAuth
/// redirect. Receives whether the authentication succeeded and a
/// human-readable error detail (not HTML-escaped — the renderer is
/// responsible for escaping if it interpolates the detail into HTML).
pub type CallbackPageRenderer = Box<dyn Fn(bool, &str) -> String + Send + Sync>;

const DEFAULT_POWERED_BY: &str = "Powered by <a href=\"https://prefix.dev\" rel=\"noopener noreferrer\" target=\"_blank\">prefix.dev</a>";

/// Text inserted into the default OAuth callback page.
#[derive(Clone, Debug)]
pub struct CallbackPageTemplate {
    /// Program name shown in the title and status text.
    pub application_name: String,
    /// Raw HTML shown in the footer. When empty, a link to the issuer domain is used.
    pub powered_by: String,
}

impl Default for CallbackPageTemplate {
    fn default() -> Self {
        Self {
            application_name: "rattler".to_string(),
            powered_by: String::new(),
        }
    }
}

/// Build a branded version of the default OAuth callback page renderer.
pub fn callback_page_renderer(
    template: CallbackPageTemplate,
    issuer_url: &str,
) -> CallbackPageRenderer {
    let domain = callback_page_domain_from_issuer(issuer_url);
    Box::new(move |success, detail| {
        default_callback_page_with_template(success, detail, &template, &domain, DEFAULT_POWERED_BY)
    })
}

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
    /// Fixed redirect URI for the auth-code flow. When `None`, rattler
    /// binds to a random localhost port. Required when the OAuth client
    /// is registered with a specific redirect URI on the `IdP` side.
    pub redirect_uri: Option<String>,
    /// Override for the User-Agent header. When `None`, defaults to
    /// `rattler/<crate version>`.
    pub user_agent: Option<String>,
    /// Override for the HTML page shown in the browser after the OAuth
    /// redirect. When `None`, [`default_callback_page`] is used. Callers
    /// (e.g. pixi, rattler-build) can supply their own branded page.
    pub callback_page: Option<CallbackPageRenderer>,
}

/// Which OAuth flow to attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    let mut config = config;
    if config.scopes.is_empty() {
        config.scopes = DEFAULT_OAUTH_SCOPES
            .iter()
            .map(|&s| s.to_string())
            .collect();
    }

    let user_agent = config.user_agent.as_deref().unwrap_or(DEFAULT_USER_AGENT);

    let reqwest_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(user_agent)
        .build()
        .map_err(OAuthError::Network)?;
    let http_client = ReqwestClient::from(reqwest_client);

    // 1. OIDC Discovery
    let endpoints = discover_endpoints(&http_client, &config.issuer_url).await?;

    let client_secret = config.client_secret.as_deref();
    let redirect_uri = config.redirect_uri.as_deref();

    let callback_page: CallbackPageRenderer = config.callback_page.unwrap_or_else(|| {
        callback_page_renderer(CallbackPageTemplate::default(), &config.issuer_url)
    });
    let callback_page: &(dyn Fn(bool, &str) -> String + Send + Sync) = &*callback_page;

    // 2. Run the appropriate flow
    let tokens = match config.flow {
        OAuthFlow::AuthCode => {
            auth_code_flow(
                &endpoints,
                &config.client_id,
                client_secret,
                &config.scopes,
                redirect_uri,
                &http_client,
                callback_page,
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
                redirect_uri,
                &http_client,
                callback_page,
            )
            .await
            {
                Ok(tokens) => tokens,
                Err(OAuthError::BrowserOpen(e)) => {
                    tracing::info!(
                        "Failed to open browser ({e}), falling back to device code flow..."
                    );
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
    http_client: &ReqwestClient,
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
    redirect_uri: Option<&str>,
    http_client: &ReqwestClient,
    callback_page: &(dyn Fn(bool, &str) -> String + Send + Sync),
) -> Result<OAuthTokens, OAuthError> {
    // If the caller pinned a redirect URI (because the IdP requires an
    // exact match against what was registered), bind there. Otherwise
    // pick a random localhost port and use that.
    let (listener, redirect_url) = if let Some(uri) = redirect_uri {
        let parsed = Url::parse(uri).map_err(OAuthError::UrlParse)?;
        let host = parsed
            .host_str()
            .ok_or_else(|| OAuthError::Authorization(format!("redirect URI has no host: {uri}")))?;
        let port = parsed.port().ok_or_else(|| {
            OAuthError::Authorization(format!("redirect URI has no explicit port: {uri}"))
        })?;
        let listener = TcpListener::bind(format!("{host}:{port}")).await?;
        (listener, uri.to_string())
    } else {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;
        (listener, format!("http://127.0.0.1:{}", local_addr.port()))
    };

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
        accept_redirect_callback(&listener, callback_page),
    )
    .await
    .map_err(|_timeout| {
        OAuthError::Authorization(
            "timed out waiting for browser authentication — please try again".into(),
        )
    })??;

    // Verify CSRF state
    if callback.state != *csrf_token.secret() {
        send_callback_response(
            &callback.stream,
            false,
            "CSRF state mismatch",
            callback_page,
        );
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
            send_callback_response(&callback.stream, true, "", callback_page);
            response
        }
        Err(e) => {
            let msg = e.to_string();
            send_callback_response(&callback.stream, false, &msg, callback_page);
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
async fn accept_redirect_callback(
    listener: &TcpListener,
    callback_page: &(dyn Fn(bool, &str) -> String + Send + Sync),
) -> Result<CallbackResult, OAuthError> {
    let (stream, _) = listener.accept().await?;

    // Convert to std TcpStream for synchronous I/O (simpler than async line
    // parsing)
    let std_stream = stream.into_std()?;
    std_stream.set_nonblocking(false)?;

    let mut reader = BufReader::new(std_stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Parse the GET request line: "GET /?code=...&state=... HTTP/1.1"
    let Some(path) = request_line.split_whitespace().nth(1) else {
        send_callback_response(
            &std_stream,
            false,
            "Invalid callback from identity provider",
            callback_page,
        );
        return Err(OAuthError::InvalidCallback);
    };

    let callback_url = match Url::parse(&format!("http://localhost{path}")) {
        Ok(url) => url,
        Err(_err) => {
            send_callback_response(
                &std_stream,
                false,
                "Invalid callback from identity provider",
                callback_page,
            );
            return Err(OAuthError::InvalidCallback);
        }
    };

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

        send_callback_response(&std_stream, false, &msg, callback_page);
        return Err(OAuthError::Authorization(msg));
    }

    let Some(code) = callback_url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())
    else {
        send_callback_response(
            &std_stream,
            false,
            "Invalid callback from identity provider",
            callback_page,
        );
        return Err(OAuthError::InvalidCallback);
    };

    let Some(state) = callback_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
    else {
        send_callback_response(
            &std_stream,
            false,
            "Invalid callback from identity provider",
            callback_page,
        );
        return Err(OAuthError::InvalidCallback);
    };

    Ok(CallbackResult {
        code,
        state,
        stream: std_stream,
    })
}

/// Escape a string for safe interpolation into HTML.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Send an HTML response to the browser on the callback stream using the
/// supplied page renderer.
fn send_callback_response(
    stream: &std::net::TcpStream,
    success: bool,
    detail: &str,
    render: &(dyn Fn(bool, &str) -> String + Send + Sync),
) {
    let response_body = render(success, detail);
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
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

fn callback_page_domain_from_issuer(issuer_url: &str) -> String {
    Url::parse(issuer_url)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))
        .unwrap_or_else(|| "prefix.dev".to_string())
}

/// Default HTML page shown in the browser after the OAuth redirect.
///
/// Callers can override this by setting [`OAuthConfig::callback_page`].
/// The returned HTML is fully self-contained (inline CSS, inline SVG) so
/// it works after the local callback server has shut down.
///
/// `detail` is treated as plain text and HTML-escaped before being
/// interpolated, so it is safe to pass raw error messages from the
/// identity provider.
pub fn default_callback_page(success: bool, detail: &str) -> String {
    default_callback_page_with_template(
        success,
        detail,
        &CallbackPageTemplate::default(),
        "prefix.dev",
        DEFAULT_POWERED_BY,
    )
}

/// Default OAuth callback page with caller-provided text.
pub fn default_callback_page_with_template(
    success: bool,
    detail: &str,
    template: &CallbackPageTemplate,
    domain: &str,
    default_powered_by: &str,
) -> String {
    const STYLES: &str = "\
        :root{color-scheme:light dark;\
        --bg:#f8fafc;--fg:#0f172a;--muted:#475569;\
        --card:#ffffff;--border:#e2e8f0;\
        --accent:#6366f1;--success:#10b981;--error:#ef4444;\
        --success-bg:rgba(16,185,129,.12);--error-bg:rgba(239,68,68,.12);}\
        @media (prefers-color-scheme:dark){:root{\
        --bg:#0b1120;--fg:#f1f5f9;--muted:#94a3b8;\
        --card:#111827;--border:#1f2937;\
        --accent:#a5b4fc;}}\
        *{box-sizing:border-box}\
        html,body{margin:0;padding:0;height:100%}\
        body{display:flex;align-items:center;justify-content:center;\
        background:var(--bg);color:var(--fg);padding:24px;\
        font-family:-apple-system,BlinkMacSystemFont,\"Segoe UI\",Roboto,\
        \"Helvetica Neue\",Arial,sans-serif;\
        font-feature-settings:\"ss01\",\"cv11\";-webkit-font-smoothing:antialiased}\
        .card{width:100%;max-width:440px;background:var(--card);\
        border:1px solid var(--border);border-radius:16px;\
        padding:40px 32px 28px;text-align:center;\
        box-shadow:0 1px 2px rgba(15,23,42,.04),0 12px 32px rgba(15,23,42,.08)}\
        .icon{width:64px;height:64px;border-radius:50%;margin:0 auto 20px;\
        display:flex;align-items:center;justify-content:center}\
        .icon.success{background:var(--success-bg);color:var(--success)}\
        .icon.error{background:var(--error-bg);color:var(--error)}\
        h1{margin:0 0 8px;font-size:22px;font-weight:600;letter-spacing:-.01em}\
        p{margin:0;color:var(--muted);font-size:15px;line-height:1.55}\
        .detail{margin-top:16px;padding:12px 14px;\
        background:var(--error-bg);border-radius:10px;\
        color:var(--error);font-family:ui-monospace,SFMono-Regular,Menlo,\
        Consolas,monospace;font-size:13px;text-align:left;\
        word-break:break-word;white-space:pre-wrap}\
        footer{margin-top:28px;padding-top:20px;border-top:1px solid var(--border);\
        color:var(--muted);font-size:12px;letter-spacing:.02em}\
        footer a{color:var(--accent);text-decoration:none;font-weight:500}\
        footer a:hover{text-decoration:underline}";

    const CHECK_SVG: &str = "<svg width=\"32\" height=\"32\" viewBox=\"0 0 24 24\" \
        fill=\"none\" stroke=\"currentColor\" stroke-width=\"2.5\" \
        stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">\
        <polyline points=\"20 6 9 17 4 12\"/></svg>";

    const CROSS_SVG: &str = "<svg width=\"32\" height=\"32\" viewBox=\"0 0 24 24\" \
        fill=\"none\" stroke=\"currentColor\" stroke-width=\"2.5\" \
        stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">\
        <line x1=\"18\" y1=\"6\" x2=\"6\" y2=\"18\"/>\
        <line x1=\"6\" y1=\"6\" x2=\"18\" y2=\"18\"/></svg>";

    let application_name = html_escape(&template.application_name);
    let domain = html_escape(domain);
    let powered_by = if template.powered_by.is_empty() {
        default_powered_by
    } else {
        &template.powered_by
    };

    let (title, kind, icon, heading, message, detail_block) = if success {
        (
            format!("Signed in to {application_name}"),
            "success",
            CHECK_SVG,
            format!("{application_name} is signed in to {domain}"),
            "Authentication completed. You can close this window and return to your terminal.",
            String::new(),
        )
    } else {
        let escaped = html_escape(detail);
        let detail_block = if escaped.is_empty() {
            String::new()
        } else {
            format!("<div class=\"detail\">{escaped}</div>")
        };
        (
            format!("{application_name} sign-in failed"),
            "error",
            CROSS_SVG,
            format!("{application_name} sign-in failed"),
            "Authentication did not complete. Please return to your terminal and try again.",
            detail_block,
        )
    };

    format!(
        "<!doctype html>\
<html lang=\"en\">\
<head>\
<meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"robots\" content=\"noindex\">\
<title>{title}</title>\
<style>{STYLES}</style>\
</head>\
<body>\
<main class=\"card\" role=\"status\" aria-live=\"polite\">\
<div class=\"icon {kind}\">{icon}</div>\
<h1>{heading}</h1>\
<p>{message}</p>\
{detail_block}\
<footer>{powered_by}</footer>\
</main>\
</body>\
</html>"
    )
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
    http_client: &ReqwestClient,
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
    if let Some(name) = claims.name()
        && let Some(n) = name.get(None)
    {
        return n.to_string();
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
    user_agent: Option<&str>,
) {
    let client = match reqwest::Client::builder()
        .user_agent(user_agent.unwrap_or(DEFAULT_USER_AGENT))
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to build HTTP client for token revocation: {e}");
            return;
        }
    };

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

#[cfg(test)]
mod tests {
    use super::{
        CallbackPageTemplate, DEFAULT_POWERED_BY, callback_page_domain_from_issuer,
        default_callback_page, default_callback_page_with_template, html_escape,
    };

    #[test]
    fn escapes_html_detail() {
        assert_eq!(html_escape("<&>\"'"), "&lt;&amp;&gt;&quot;&#x27;");
    }

    #[test]
    fn default_callback_page_escapes_error_detail() {
        let page = default_callback_page(false, "<script>alert('x')</script>");

        assert!(page.contains("&lt;script&gt;alert(&#x27;x&#x27;)&lt;/script&gt;"));
        assert!(!page.contains("<script>alert"));
    }

    #[test]
    fn default_callback_page_hides_empty_error_detail() {
        let page = default_callback_page(false, "");

        assert!(!page.contains("class=\"detail\""));
    }

    #[test]
    fn default_callback_page_uses_template_text() {
        let page = default_callback_page_with_template(
            true,
            "",
            &CallbackPageTemplate {
                application_name: "pixi".to_string(),
                powered_by: "built by <strong>example</strong>".to_string(),
            },
            "example.com",
            "Powered by example.com",
        );

        assert!(page.contains("pixi is signed in to example.com"));
        assert!(page.contains("built by <strong>example</strong>"));
    }

    #[test]
    fn callback_page_uses_issuer_domain() {
        let domain = callback_page_domain_from_issuer("https://login.example.com/realms/prefix");

        assert_eq!(domain, "login.example.com");
    }

    #[test]
    fn empty_powered_by_uses_prefix_dev() {
        let page = default_callback_page_with_template(
            true,
            "",
            &CallbackPageTemplate {
                application_name: "pixi".to_string(),
                powered_by: String::new(),
            },
            "login.example.com",
            DEFAULT_POWERED_BY,
        );

        assert!(page.contains("pixi is signed in to login.example.com"));
        assert!(page.contains("https://prefix.dev"));
        assert!(page.contains(">prefix.dev</a>"));
    }
}
