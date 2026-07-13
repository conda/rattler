//! This module contains CLI common entrypoint for authentication.

#[cfg(feature = "oauth")]
pub mod oauth;

use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use clap::Parser;
use console::style;
use jiff::Timestamp;
use rattler_networking::{
    Authentication, AuthenticationStorage, authentication_storage::AuthenticationStorageError,
};
use reqwest::{Client, header::CONTENT_TYPE};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror;
use url::Url;

/// Default `User-Agent` header sent to remote endpoints (OAuth providers,
/// prefix.dev validation, token revocation) when the caller passes no
/// override. Library consumers (pixi etc.) typically pass their own value.
pub const DEFAULT_USER_AGENT: &str = concat!("rattler/", env!("CARGO_PKG_VERSION"));

/// Command line arguments that contain authentication data
#[derive(Parser, Debug)]
struct LoginArgs {
    /// The host to authenticate with (e.g. prefix.dev)
    host: String,

    // -- Token / Basic auth --
    /// The token to use (for authentication with prefix.dev)
    #[clap(long, help_heading = "Token / Basic Authentication")]
    token: Option<String>,

    /// The username to use (for basic HTTP authentication)
    #[clap(long, help_heading = "Token / Basic Authentication")]
    username: Option<String>,

    /// The password to use (for basic HTTP authentication)
    #[clap(long, help_heading = "Token / Basic Authentication")]
    password: Option<String>,

    /// The token to use on anaconda.org / quetz authentication
    #[clap(long, help_heading = "Token / Basic Authentication")]
    conda_token: Option<String>,

    // -- S3 --
    /// The S3 access key ID
    #[clap(long, requires_all = ["s3_secret_access_key"], conflicts_with_all = ["token", "username", "password", "conda_token"], help_heading = "S3 Authentication")]
    s3_access_key_id: Option<String>,

    /// The S3 secret access key
    #[clap(long, requires_all = ["s3_access_key_id"], help_heading = "S3 Authentication")]
    s3_secret_access_key: Option<String>,

    /// The S3 session token
    #[clap(long, requires_all = ["s3_access_key_id"], help_heading = "S3 Authentication")]
    s3_session_token: Option<String>,

    // -- OAuth/OIDC --
    /// Use OAuth/OIDC authentication
    #[cfg(feature = "oauth")]
    #[clap(long, conflicts_with_all = ["token", "username", "password", "conda_token", "s3_access_key_id"], help_heading = "OAuth/OIDC Authentication")]
    oauth: bool,

    /// OIDC issuer URL (defaults to <https://{host>})
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", help_heading = "OAuth/OIDC Authentication")]
    oauth_issuer_url: Option<String>,

    /// OAuth client ID (defaults to "rattler")
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", help_heading = "OAuth/OIDC Authentication")]
    oauth_client_id: Option<String>,

    /// OAuth client secret (for confidential clients)
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", help_heading = "OAuth/OIDC Authentication")]
    oauth_client_secret: Option<String>,

    /// OAuth flow: device-code (default), auth-code, auto
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", value_parser = ["device-code", "auth-code", "auto"], help_heading = "OAuth/OIDC Authentication")]
    oauth_flow: Option<String>,

    /// Additional OAuth scopes to request (repeatable)
    #[cfg(feature = "oauth")]
    #[clap(
        long = "oauth-scope",
        requires = "oauth",
        help_heading = "OAuth/OIDC Authentication"
    )]
    oauth_scopes: Vec<String>,

    /// OAuth redirect URI (defaults to a random localhost port). Set
    /// this when the OAuth client on the `IdP` side is registered with
    /// a specific redirect URI such as `http://127.0.0.1:8000/auth/oidc`.
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", help_heading = "OAuth/OIDC Authentication")]
    oauth_redirect_uri: Option<String>,

    /// User-Agent header used for requests
    #[clap(long)]
    user_agent: Option<String>,
}

#[derive(Parser, Debug)]
struct LogoutArgs {
    /// The host to remove authentication for. With `auth-interactive`
    /// enabled, omit this (and `--all`) to pick interactively.
    #[cfg_attr(
        not(feature = "auth-interactive"),
        clap(required_unless_present = "all", conflicts_with = "all")
    )]
    #[cfg_attr(feature = "auth-interactive", clap(conflicts_with = "all"))]
    host: Option<String>,

    /// Remove every stored authentication entry (revoking OAuth tokens for each)
    #[clap(long)]
    all: bool,
}

#[derive(Parser, Debug)]
struct StatusArgs {
    /// Show endpoint URLs, client ID, and other IdP-introspection fields
    /// that are only useful for debugging.
    // NOTE: this intentionally avoids the `--verbose`/`-v` name. `Args` is
    // embedded as a subcommand in larger CLIs (pixi, the `rattler` binary, ...)
    // that define their own global `--verbose` logging flag. Sharing the name
    // makes clap either merge the two args (panicking on a type mismatch) or
    // reject them as duplicate long names — see prefix-dev/pixi#6466.
    #[clap(long)]
    details: bool,
}

#[derive(Parser, Debug)]
#[allow(clippy::large_enum_variant)]
enum Subcommand {
    /// Store authentication information for a given host
    Login(LoginArgs),
    /// Remove authentication information for a given host
    Logout(LogoutArgs),
    /// Show stored authentication entries and non-secret token metadata
    Status(StatusArgs),
}

/// Login to prefix.dev or anaconda.org servers to access private channels
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(subcommand)]
    subcommand: Subcommand,
}

/// Authentication errors that can be returned by the `AuthenticationCLIError`
#[derive(thiserror::Error, Debug)]
pub enum AuthenticationCLIError {
    /// An error occurred when the input repository URL is parsed
    #[error("Failed to parse the URL")]
    ParseUrlError(#[from] url::ParseError),

    /// Basic authentication needs a username and a password. The password is
    /// missing here.
    #[error("Password must be provided when using basic authentication")]
    MissingPassword,

    /// Authentication has not been provided in the input parameters.
    #[error("No authentication method provided")]
    NoAuthenticationMethod,

    /// Bad authentication method when using prefix.dev
    #[error("Authentication with prefix.dev requires a token. Use `--token` to provide one")]
    PrefixDevBadMethod,

    /// Bad authentication method when using anaconda.org
    #[error(
        "Authentication with anaconda.org requires a conda token. Use `--conda-token` to provide one"
    )]
    AnacondaOrgBadMethod,

    /// Bad authentication method when using S3
    #[error(
        "Authentication with S3 requires a S3 access key ID and a secret access key. Use `--s3-access-key-id` and `--s3-secret-access-key` to provide them"
    )]
    S3BadMethod,

    // TODO: rework this
    /// Wrapper for errors that are generated from the underlying storage system
    /// (keyring or file system)
    #[error("Failed to interact with the authentication storage system")]
    AnyhowError(#[from] anyhow::Error),

    /// Wrapper for errors that are generated from the underlying storage system
    /// (keyring or file system)
    #[error("Failed to interact with the authentication storage system")]
    AuthenticationStorageError(#[from] AuthenticationStorageError),

    /// General http request error
    #[error("General http request error")]
    ReqwestError(#[from] reqwest::Error),

    /// Network access was requested while offline mode was enabled.
    #[error("network access is disabled by offline mode")]
    Offline,

    /// JSON parsing failed
    #[error("Failed to parse JSON: {0}")]
    JsonParseError(String),

    /// Token is unauthorized or invalid
    #[error("Unauthorized or invalid token")]
    UnauthorizedToken,

    /// No stored credentials were found for the requested host.
    #[error("No stored credentials found for {0}")]
    NotLoggedIn(String),

    /// Interactive logout was requested but the process isn't attached to a TTY.
    #[cfg(feature = "auth-interactive")]
    #[error("Interactive logout requires a TTY. Pass a host or `--all` instead.")]
    NotInteractive,

    /// The interactive picker failed (e.g. user interrupted, terminal error).
    #[cfg(feature = "auth-interactive")]
    #[error("Interactive selection failed: {0}")]
    Interactive(String),

    /// OAuth error
    #[cfg(feature = "oauth")]
    #[error(transparent)]
    OAuthError(#[from] oauth::OAuthError),
}

/// Normalize a user-supplied host into its canonical hostname form.
fn normalize_login_host(host: &str) -> String {
    let host = host.trim_start_matches("*.");

    // Try parsing as-is first (handles inputs like `https://prefix.dev`).
    // Only accept the result if it actually yielded a hostname — not every
    // parse-successful string contains a host component.
    if let Some(h) = url::Url::parse(host)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
    {
        return h;
    }

    // Fall back to prepending a scheme (handles bare `prefix.dev`,
    // `prefix.dev/`, `localhost:8080`, etc.).
    url::Url::parse(&format!("https://{host}"))
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| host.trim_end_matches('/').to_string())
}

/// prefix.dev's default channel scopes
#[cfg(feature = "oauth")]
const PREFIX_DEV_OAUTH_SCOPES: &[&str] = &[
    "openid",
    "profile",
    "offline_access",
    "channel:read",
    "channel:upload",
];

/// Built-in OAuth defaults for a known host.
///
/// Returned by [`default_oauth_config_for_host`] for hosts where rattler
/// ships an out-of-the-box OAuth configuration. Carries everything needed
/// to start a login flow without the user passing any flags.
#[cfg(feature = "oauth")]
struct DefaultOAuthConfig {
    issuer_url: String,
    client_id: String,
    scopes: Vec<String>,
    redirect_uri: Option<String>,
}

/// Returns the built-in OAuth configuration for a host, if rattler ships one.
#[cfg(feature = "oauth")]
fn default_oauth_config_for_host(host: &str) -> Option<DefaultOAuthConfig> {
    let normalized = normalize_login_host(host);

    if !(normalized == "prefix.dev" || normalized.ends_with(".prefix.dev")) {
        return None;
    }

    Some(DefaultOAuthConfig {
        issuer_url: ensure_url_scheme(host),
        client_id: "rattler".to_string(),
        scopes: PREFIX_DEV_OAUTH_SCOPES
            .iter()
            .map(|&s| s.to_string())
            .collect(),
        redirect_uri: None,
    })
}

/// Returns the built-in OAuth config for an implicit (flag-less) login —
/// i.e. when the user passed no explicit auth method and the host ships
/// an out-of-the-box OAuth configuration. The presence of `Some` is the
/// signal that `login()` should fall back to OAuth.
#[cfg(feature = "oauth")]
fn default_oauth_for_login(args: &LoginArgs) -> Option<DefaultOAuthConfig> {
    let no_explicit_method = args.token.is_none()
        && args.username.is_none()
        && args.password.is_none()
        && args.conda_token.is_none()
        && args.s3_access_key_id.is_none();

    if !no_explicit_method {
        return None;
    }

    default_oauth_config_for_host(&args.host)
}

fn get_url(url: &str) -> Result<String, AuthenticationCLIError> {
    // parse as url and extract host without scheme or port
    let host = if url.contains("://") {
        url::Url::parse(url)?.host_str().unwrap().to_string()
    } else {
        url.to_string()
    };

    let host = if host.matches('.').count() == 1 {
        // use wildcard for top-level domains
        format!("*.{host}")
    } else {
        host
    };

    Ok(host)
}

/// Ensure a user-supplied `host` is a fully-qualified URL by prepending
/// `https://` when it has no scheme.
fn ensure_url_scheme(host: &str) -> String {
    if host.contains("://") {
        host.to_string()
    } else {
        format!("https://{host}")
    }
}

/// Result of prefix.dev token validation
#[derive(Debug, PartialEq)]
pub enum ValidationResult {
    /// Token is valid and associated with this username
    Valid(String, Url),
    /// Token is invalid or unauthorized
    Invalid,
}

/// Authenticate with a host using the provided credentials.
///
/// This function validates the authentication method based on the host and
/// stores the credentials if successful. For prefix.dev hosts, it validates the
/// token by making a GraphQL API call.
#[cfg(test)]
async fn login(
    args: LoginArgs,
    storage: AuthenticationStorage,
) -> Result<(), AuthenticationCLIError> {
    login_with_offline(args, storage, false).await
}

async fn login_with_offline(
    args: LoginArgs,
    storage: AuthenticationStorage,
    offline: bool,
) -> Result<(), AuthenticationCLIError> {
    // explicit `--oauth` *or* no explicit method on an OAuth-capable host
    #[cfg(feature = "oauth")]
    {
        let auto_default = default_oauth_for_login(&args);
        if args.oauth || auto_default.is_some() {
            if offline {
                return Err(AuthenticationCLIError::Offline);
            }
            if !args.oauth {
                eprintln!(
                    "No credentials provided; using OAuth device code login for {}.",
                    args.host
                );
            }

            // Reuse the implicit-default config when present; otherwise
            // (`--oauth` was set explicitly) fall back to a fresh lookup.
            let host_default = auto_default.or_else(|| default_oauth_config_for_host(&args.host));

            let issuer_url = args
                .oauth_issuer_url
                .or_else(|| host_default.as_ref().map(|c| c.issuer_url.clone()))
                .unwrap_or_else(|| ensure_url_scheme(&args.host));

            let client_id = args
                .oauth_client_id
                .or_else(|| host_default.as_ref().map(|c| c.client_id.clone()))
                .unwrap_or_else(|| "rattler".to_string());

            let flow = match args.oauth_flow.as_deref() {
                Some("auth-code") => oauth::OAuthFlow::AuthCode,
                Some("auto") => oauth::OAuthFlow::Auto,
                _ => oauth::OAuthFlow::DeviceCode,
            };

            let redirect_uri = args
                .oauth_redirect_uri
                .or_else(|| host_default.as_ref().and_then(|c| c.redirect_uri.clone()));

            let scopes: std::collections::HashSet<String> = if !args.oauth_scopes.is_empty() {
                args.oauth_scopes.into_iter().collect()
            } else if let Some(default) = host_default {
                default.scopes.into_iter().collect()
            } else {
                oauth::DEFAULT_OAUTH_SCOPES
                    .iter()
                    .map(|&s| s.to_string())
                    .collect()
            };

            let config = oauth::OAuthConfig {
                issuer_url,
                client_id,
                client_secret: args.oauth_client_secret,
                flow,
                scopes,
                redirect_uri,
                user_agent: args.user_agent,
                callback_page: None,
            };

            let auth = oauth::perform_oauth_login(config).await?;
            // Normalize the host so that `prefix.dev` and `prefix.dev/` (and
            // any `https://...` form) write to the same storage key
            let host = normalize_login_host(&args.host);
            storage.store(&host, &auth)?;
            eprintln!("Credentials stored for {host}.");
            return Ok(());
        }
    }

    let auth = if let Some(conda_token) = args.conda_token {
        Authentication::CondaToken(conda_token)
    } else if let Some(username) = args.username {
        if let Some(password) = args.password {
            Authentication::BasicHTTP { username, password }
        } else {
            return Err(AuthenticationCLIError::MissingPassword);
        }
    } else if let Some(token) = args.token {
        Authentication::BearerToken(token)
    } else if let (Some(access_key_id), Some(secret_access_key)) =
        (args.s3_access_key_id, args.s3_secret_access_key)
    {
        let session_token = args.s3_session_token;
        Authentication::S3Credentials {
            access_key_id,
            secret_access_key,
            session_token,
        }
    } else {
        return Err(AuthenticationCLIError::NoAuthenticationMethod);
    };

    if args.host.contains("prefix.dev") && !matches!(auth, Authentication::BearerToken(_)) {
        return Err(AuthenticationCLIError::PrefixDevBadMethod);
    }

    if args.host.contains("anaconda.org") && !matches!(auth, Authentication::CondaToken(_)) {
        return Err(AuthenticationCLIError::AnacondaOrgBadMethod);
    }

    if args.host.contains("s3://") && !matches!(auth, Authentication::S3Credentials { .. })
        || matches!(auth, Authentication::S3Credentials { .. }) && !args.host.contains("s3://")
    {
        return Err(AuthenticationCLIError::S3BadMethod);
    }

    let host = get_url(&args.host)?;
    eprintln!("Authenticating with {host} using {} method", auth.method());

    // Only validate token for prefix.dev
    if args.host.contains("prefix.dev") {
        if offline {
            return Err(AuthenticationCLIError::Offline);
        }

        // Extract the token from BearerToken
        let token = match &auth {
            Authentication::BearerToken(t) => t,
            _ => return Err(AuthenticationCLIError::PrefixDevBadMethod),
        };

        // Validate the token using the extracted function
        match validate_prefix_dev_token(token, &args.host, args.user_agent.as_deref()).await? {
            ValidationResult::Valid(username, url) => {
                println!(
                    "✅ Token is valid. Logged into {url} as \"{username}\". Storing credentials..."
                );
                // Store the authentication
                storage.store(&host, &auth)?;
            }
            ValidationResult::Invalid => {
                return Err(AuthenticationCLIError::UnauthorizedToken);
            }
        }
    } else {
        // For non-prefix.dev hosts, store directly without validation
        storage.store(&host, &auth)?;
    }
    Ok(())
}

/// Validates a token with prefix.dev by making a GraphQL API call
///
/// Returns `Ok(true)` if the token is valid, `Ok(false)` if invalid,
/// or `Err` if there was a network/parsing error
async fn validate_prefix_dev_token(
    token: &str,
    host: &str,
    user_agent: Option<&str>,
) -> Result<ValidationResult, AuthenticationCLIError> {
    let prefix_url = if let Ok(env_var) = std::env::var("PREFIX_DEV_API_URL") {
        // If env var is set, parse it as a full URL
        Url::parse(&env_var).expect("PREFIX_DEV_API_URL must be a valid URL")
    } else {
        // Strip wildcard if given
        let host = host.replace("*.", "");

        let host_url = Url::parse(&ensure_url_scheme(&host))?;

        let host_url = host_url.host_str().unwrap_or("prefix.dev");
        // Strip "repo." prefix if present
        let host_url = host_url.strip_prefix("repo.").unwrap_or(host_url);

        Url::parse(&format!("https://{host_url}")).expect("constructed url must be valid")
    };

    let body = json!({
        "query": "query { viewer { login } }"
    });

    let client = Client::builder()
        .user_agent(user_agent.unwrap_or(DEFAULT_USER_AGENT))
        .build()?;
    let response = client
        .post(prefix_url.join("api/graphql").expect("must be valid"))
        .bearer_auth(token)
        .header(CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    let text = response.text().await?;

    // Parse JSON
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| AuthenticationCLIError::JsonParseError(e.to_string()))?;

    // Check if viewer is null (invalid token) or contains user data (valid token)
    match &json["data"]["viewer"] {
        serde_json::Value::Null => Ok(ValidationResult::Invalid),
        viewer_data => {
            if let Some(username) = viewer_data["login"].as_str() {
                Ok(ValidationResult::Valid(username.to_string(), prefix_url))
            } else {
                Ok(ValidationResult::Invalid)
            }
        }
    }
}

/// Show an interactive `MultiSelect` picker so the user can pick which stored
/// entries to log out from. Returns the chosen subset.
///
/// Bails with `NotLoggedIn("any host")` on an empty storage and prints a
/// helpful note when stdin/stdout isn't a TTY (since dialoguer would just
/// hang or error obscurely in that case).
/// Returns `Ok(None)` when the user aborts the picker (Ctrl-C / SIGINT); the
/// caller treats that as a quiet cancel rather than an error.
#[cfg(feature = "auth-interactive")]
async fn interactive_pick(
    entries: Vec<rattler_networking::authentication_storage::storage::LazyListedEntry>,
) -> Result<
    Option<Vec<rattler_networking::authentication_storage::storage::LazyListedEntry>>,
    AuthenticationCLIError,
> {
    use std::io::IsTerminal;

    if entries.is_empty() {
        // Caller's empty-match branch turns this into a friendly no-op.
        return Ok(Some(Vec::new()));
    }

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(AuthenticationCLIError::NotInteractive);
    }

    // Auth method isn't shown here — it would require reading the secret,
    // which on macOS prompts the keychain once per entry. Host + source is
    // enough to disambiguate every entry in practice.
    let items: Vec<String> = entries
        .iter()
        .map(|e| {
            let shadowed = if e.active { "" } else { " (shadowed)" };
            format!("{} ({}){}", e.host, e.source, shadowed)
        })
        .collect();

    // dialoguer hides the cursor for the duration of the picker but only
    // restores it on its Enter/Esc/quit paths. The blocking picker runs on a
    // dedicated thread so we can race it against Ctrl-C: tokio's SIGINT handler
    // replaces the default (process-killing) one, giving us a chance to put the
    // cursor back. The `CursorGuard` covers every return path of the picker
    // itself (normal exit, error, or the SIGINT that `console` re-raises mid
    // read); the explicit `show_cursor()` in the Ctrl-C branch covers the case
    // where the signal future wins the race before the guard runs.
    let handle = tokio::task::spawn_blocking(move || {
        struct CursorGuard;
        impl Drop for CursorGuard {
            fn drop(&mut self) {
                let _ = console::Term::stdout().show_cursor();
            }
        }
        let _guard = CursorGuard;

        match dialoguer::MultiSelect::new()
            .with_prompt("Select credentials to log out from (space to toggle, enter to confirm)")
            .items(&items)
            .interact()
        {
            Ok(selection) => Ok(Some(
                selection.into_iter().map(|i| entries[i].clone()).collect(),
            )),
            // `console` reports Ctrl-C as an `Interrupted` IO error. Treat it as
            // a cancel, not a failure, so we don't surface a scary message.
            Err(e) => {
                let io: std::io::Error = e.into();
                if io.kind() == std::io::ErrorKind::Interrupted {
                    Ok(None)
                } else {
                    Err(AuthenticationCLIError::Interactive(io.to_string()))
                }
            }
        }
    });

    tokio::select! {
        res = handle => res.map_err(|e| AuthenticationCLIError::Interactive(e.to_string()))?,
        _ = tokio::signal::ctrl_c() => {
            let _ = console::Term::stdout().show_cursor();
            Ok(None)
        }
    }
}

/// Candidate storage keys to look up for a user-supplied host.
///
/// `login` historically writes under two different canonical forms:
/// - the OAuth path uses [`normalize_login_host`] (e.g. `prefix.dev`)
/// - the token/basic/S3 path uses [`get_url`] (e.g. `*.prefix.dev`)
///
/// To stay backwards-compatible with both, logout matches entries against
/// every form the host could have been stored under.
fn logout_candidate_keys(host: &str) -> Result<Vec<String>, AuthenticationCLIError> {
    let mut keys = vec![normalize_login_host(host), get_url(host)?];
    // Also accept the raw input — covers manually written entries (e.g.
    // pre-existing `RATTLER_AUTH_FILE` content) that don't match either
    // derived form.
    keys.push(host.to_string());
    keys.sort();
    keys.dedup();
    Ok(keys)
}

#[cfg(test)]
async fn logout(
    args: LogoutArgs,
    storage: AuthenticationStorage,
) -> Result<(), AuthenticationCLIError> {
    logout_with_offline(args, storage, false).await
}

async fn logout_with_offline(
    args: LogoutArgs,
    storage: AuthenticationStorage,
    offline: bool,
) -> Result<(), AuthenticationCLIError> {
    // Enumerate hosts without reading secrets — important on macOS where each
    // keychain read prompts the user. We fetch credentials lazily, once per
    // entry we actually touch.
    let lazy_entries = storage.list_keys_with_sources()?;

    let targets: Vec<_> = match (args.all, args.host.as_deref()) {
        (true, _) => lazy_entries,
        (false, Some(host)) => {
            let candidates = logout_candidate_keys(host)?;
            lazy_entries
                .into_iter()
                .filter(|e| candidates.contains(&e.host))
                .collect()
        }
        // With `auth-interactive`, no host + no --all means "let the user
        // pick". Without the feature, clap rejects this combo before we get
        // here; the fall-through treats it like --all for programmatic
        // callers who construct LogoutArgs directly.
        (false, None) => {
            #[cfg(feature = "auth-interactive")]
            {
                match interactive_pick(lazy_entries).await? {
                    Some(picked) => picked,
                    // User aborted the picker (Ctrl-C) — exit quietly.
                    None => return Ok(()),
                }
            }
            #[cfg(not(feature = "auth-interactive"))]
            {
                lazy_entries
            }
        }
    };

    if targets.is_empty() {
        // No-op cases:
        // - `--all` with nothing stored
        // - interactive picker confirmed with nothing selected (host is None
        //   because clap allows that under `auth-interactive`)
        // The only real "error" case is an explicit host that doesn't match.
        if args.all || args.host.is_none() {
            eprintln!("No credentials to remove.");
            return Ok(());
        }
        return Err(AuthenticationCLIError::NotLoggedIn(args.host.unwrap()));
    }

    // Single fetch per target — for an entry stored in the macOS keychain
    // this is one prompt per entry being touched, not per entry in storage.
    for entry in &targets {
        let Some(auth) = storage.get_entry(&entry.host, &entry.source)? else {
            // Disappeared between the lazy list and now (concurrent edit, or
            // a backend that lies about its keys). Skip.
            continue;
        };

        #[cfg(feature = "oauth")]
        if let Authentication::OAuth {
            access_token,
            refresh_token,
            revocation_endpoint: Some(revocation_endpoint),
            client_id,
            ..
        } = &auth
        {
            if offline {
                eprintln!(
                    "Skipping OAuth token revocation for {} ({}) because offline mode is enabled",
                    entry.host, entry.source
                );
            } else {
                eprintln!(
                    "Revoking OAuth tokens for {} ({})",
                    entry.host, entry.source
                );
                oauth::revoke_tokens(
                    revocation_endpoint,
                    access_token,
                    refresh_token.as_deref(),
                    client_id,
                    None,
                )
                .await;
            }
        }
        // Silence unused-variable warning when oauth is off.
        let _ = auth;

        eprintln!(
            "Removing authentication for {} ({})",
            entry.host, entry.source
        );
        storage.delete_entry(&entry.host, &entry.source)?;
    }

    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TokenMetadata {
    expires_at: Option<i64>,
    scopes: Vec<String>,
    issuer: Option<String>,
    subject: Option<String>,
    audience: Vec<String>,
}

fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn string_or_string_array(value: &Value) -> Vec<String> {
    match value {
        Value::String(value) => vec![value.clone()],
        Value::Array(values) => values
            .iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn token_metadata(token: &str) -> Option<TokenMetadata> {
    let claims = jwt_claims(token)?;

    let mut scopes = claims
        .get("scope")
        .and_then(Value::as_str)
        .map(|scope| {
            scope
                .split_whitespace()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    scopes.extend(
        claims
            .get("scp")
            .map(string_or_string_array)
            .unwrap_or_default(),
    );
    scopes.sort();
    scopes.dedup();

    Some(TokenMetadata {
        expires_at: claims.get("exp").and_then(Value::as_i64),
        scopes,
        issuer: claims
            .get("iss")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        subject: claims
            .get("sub")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        audience: claims
            .get("aud")
            .map(string_or_string_array)
            .unwrap_or_default(),
    })
}

fn now_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn format_timestamp(timestamp: i64) -> String {
    Timestamp::from_second(timestamp).map_or_else(
        |_| format!("unix timestamp {timestamp}"),
        |ts| ts.strftime("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )
}

fn format_validity(expires_at: Option<i64>, now: i64) -> String {
    let Some(expires_at) = expires_at else {
        return "unknown (no expiry metadata)".to_string();
    };

    let timestamp = format_timestamp(expires_at);
    if expires_at <= now {
        let elapsed = Duration::from_secs((now - expires_at) as u64);
        format!(
            "expired at {timestamp} ({} ago)",
            humantime::format_duration(elapsed)
        )
    } else {
        let remaining = Duration::from_secs((expires_at - now) as u64);
        format!(
            "valid until {timestamp} (in {})",
            humantime::format_duration(remaining)
        )
    }
}

fn print_token_metadata(metadata: Option<&TokenMetadata>, verbose: bool) {
    let Some(metadata) = metadata else {
        return;
    };

    if !metadata.scopes.is_empty() {
        let scopes = metadata
            .scopes
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  - Token scopes: {scopes}");
    }
    if !verbose {
        // Issuer/audience/subject are JWT introspection fields — useful for
        // debugging, noisy for a typical "am I logged in?" check.
        return;
    }
    if let Some(issuer) = &metadata.issuer {
        println!("  - Issuer: {issuer}");
    }
    if !metadata.audience.is_empty() {
        println!("  - Audience: {}", metadata.audience.join(", "));
    }
    if let Some(subject) = &metadata.subject {
        println!("  - Subject: {subject}");
    }
}

fn print_authentication_status(
    host: &str,
    auth: &Authentication,
    source: &str,
    active: bool,
    account: Option<&str>,
    now: i64,
    verbose: bool,
) {
    if active {
        println!("{host}");
    } else {
        // Shadowed entry — `get()` would return a different backend's copy
        // for this host. Dim the heading so it's visually subordinate but
        // still useful for cleanup.
        println!("{} {}", host, style("(shadowed)").dim());
    }

    // Header line. For verified prefix.dev entries (account known) we mirror
    // `gh auth status` and lead with a "✓ Logged in to ..." line. For
    // anything else we just report where the entry came from.
    match account {
        Some(account) => println!(
            "  {} Logged in to {host} account {account} ({source})",
            style("✓").green()
        ),
        None => println!("  - Source: {source}"),
    }
    println!("  - Method: {}", auth.method());

    match auth {
        Authentication::BearerToken(token) | Authentication::CondaToken(token) => {
            let metadata = token_metadata(token);
            println!(
                "  - Token validity: {}",
                format_validity(
                    metadata.as_ref().and_then(|metadata| metadata.expires_at),
                    now
                )
            );
            print_token_metadata(metadata.as_ref(), verbose);
        }
        Authentication::OAuth {
            access_token,
            refresh_token,
            expires_at,
            token_endpoint,
            revocation_endpoint,
            client_id,
        } => {
            let metadata = token_metadata(access_token);
            // OAuth's `expires_at` always refers to the short-lived ACCESS
            // token (RFC 6749 §5.1). Refresh tokens are separate and the
            // standard response doesn't expose their expiry — they're
            // typically valid for days/weeks and we only learn they're gone
            // when a refresh fails.
            println!(
                "  - Access token expires: {}",
                format_validity(
                    expires_at
                        .or_else(|| metadata.as_ref().and_then(|metadata| metadata.expires_at)),
                    now,
                )
            );
            println!(
                "  - Refresh token: {}",
                if refresh_token.is_some() {
                    "yes (lifetime not exposed by the provider)"
                } else {
                    "no"
                }
            );
            if verbose {
                println!("  - Client ID: {client_id}");
                println!("  - Token endpoint: {token_endpoint}");
                if let Some(revocation_endpoint) = revocation_endpoint {
                    println!("  - Revocation endpoint: {revocation_endpoint}");
                }
            }
            print_token_metadata(metadata.as_ref(), verbose);
        }
        Authentication::BasicHTTP { username, .. } => {
            println!("  - Username: {username}");
        }
        Authentication::S3Credentials { session_token, .. } => {
            println!(
                "  - Session token: {}",
                if session_token.is_some() {
                    "present"
                } else {
                    "none"
                }
            );
        }
    }
}

/// Returns true if `host` belongs to the prefix.dev family (e.g. `prefix.dev`,
/// `repo.prefix.dev`, `*.prefix.dev`). Used to decide whether the status
/// command should look up the account name via prefix.dev's GraphQL API.
fn is_prefix_dev_host(host: &str) -> bool {
    let normalized = normalize_login_host(host);
    normalized == "prefix.dev" || normalized.ends_with(".prefix.dev")
}

/// Extract a bearer-style token from an authentication entry if one is
/// available — i.e. something that can be sent as `Authorization: Bearer …`
/// to prefix.dev's GraphQL API.
fn bearer_for_prefix_dev(auth: &Authentication) -> Option<&str> {
    match auth {
        Authentication::BearerToken(token) => Some(token.as_str()),
        Authentication::OAuth { access_token, .. } => Some(access_token.as_str()),
        _ => None,
    }
}

/// Best-effort lookup of the prefix.dev account name for an entry. Returns
/// `None` on any failure (network error, invalid token, non-prefix.dev host)
/// since this is a display-only enrichment for `auth status`.
async fn lookup_prefix_dev_account(host: &str, auth: &Authentication) -> Option<String> {
    if !is_prefix_dev_host(host) {
        return None;
    }
    let token = bearer_for_prefix_dev(auth)?;
    match validate_prefix_dev_token(token, host, None).await {
        Ok(ValidationResult::Valid(username, _)) => Some(username),
        _ => None,
    }
}

async fn status(
    args: StatusArgs,
    storage: AuthenticationStorage,
) -> Result<(), AuthenticationCLIError> {
    let entries = storage.list_with_sources()?;

    if entries.is_empty() {
        println!("No stored authentication entries found.");
        return Ok(());
    }

    // Only call prefix.dev's API for entries that `get()` would actually
    // return — there's no point validating shadowed copies.
    let accounts = futures::future::join_all(entries.iter().map(|entry| async {
        if entry.active {
            lookup_prefix_dev_account(&entry.host, &entry.auth).await
        } else {
            None
        }
    }))
    .await;

    println!("Stored authentication entries:");
    let now = now_unix_timestamp();
    for (index, (entry, account)) in entries.iter().zip(accounts.iter()).enumerate() {
        if index > 0 {
            println!();
        }
        print_authentication_status(
            &entry.host,
            &entry.auth,
            &entry.source,
            entry.active,
            account.as_deref(),
            now,
            args.details,
        );
    }

    Ok(())
}

/// CLI entrypoint for authentication
pub async fn execute(args: Args) -> Result<(), AuthenticationCLIError> {
    execute_with_offline(args, false).await
}

/// CLI entrypoint for authentication with optional offline mode.
pub async fn execute_with_offline(args: Args, offline: bool) -> Result<(), AuthenticationCLIError> {
    let storage = AuthenticationStorage::from_env_and_defaults()?;

    match args.subcommand {
        Subcommand::Login(args) => login_with_offline(args, storage, offline).await,
        Subcommand::Logout(args) => logout_with_offline(args, storage, offline).await,
        Subcommand::Status(args) => status(args, storage).await,
    }
}

#[cfg(test)]
mod tests {
    use mockito::Server;
    use rattler_networking::{
        AuthenticationStorage, authentication_storage::backends::memory::MemoryStorage,
    };
    use serde_json::json;
    use temp_env::async_with_vars;
    use tempfile::TempDir;

    use super::*;

    // Helper function to create a test authentication storage
    fn create_test_storage() -> (AuthenticationStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(std::sync::Arc::new(MemoryStorage::new()));
        (storage, temp_dir)
    }

    // Helper function to create LoginArgs
    fn create_login_args(host: &str) -> LoginArgs {
        LoginArgs {
            host: host.to_string(),
            token: None,
            username: None,
            password: None,
            conda_token: None,
            s3_access_key_id: None,
            s3_secret_access_key: None,
            s3_session_token: None,
            #[cfg(feature = "oauth")]
            oauth: false,
            #[cfg(feature = "oauth")]
            oauth_issuer_url: None,
            #[cfg(feature = "oauth")]
            oauth_client_id: None,
            #[cfg(feature = "oauth")]
            oauth_client_secret: None,
            #[cfg(feature = "oauth")]
            oauth_flow: None,
            #[cfg(feature = "oauth")]
            oauth_scopes: vec![],
            #[cfg(feature = "oauth")]
            oauth_redirect_uri: None,
            user_agent: None,
        }
    }

    fn unsigned_jwt(payload: Value) -> String {
        let header = json!({ "alg": "none" });
        format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )
    }

    // Auth `Args` must be embeddable as a subcommand of a larger CLI that
    // defines its own `global = true` `--verbose` flag, without clashing.
    // pixi uses a `u8` (count) flag, the `rattler` binary a `bool` one — both
    // previously broke `auth status` (panic on type mismatch or a duplicate
    // long-name assertion). See prefix-dev/pixi#6466.
    #[test]
    fn auth_embeds_under_global_verbose_flag() {
        #[derive(Parser, Debug)]
        struct CountParent {
            #[clap(short, long, action = clap::ArgAction::Count, global = true)]
            verbose: u8,
            #[clap(subcommand)]
            command: ParentCommand,
        }

        #[derive(Parser, Debug)]
        struct BoolParent {
            #[clap(short, long, global = true)]
            verbose: bool,
            #[clap(subcommand)]
            command: ParentCommand,
        }

        #[derive(Parser, Debug)]
        enum ParentCommand {
            Auth(super::Args),
        }

        // `try_parse_from` panics (rather than returning `Err`) on the clashing
        // definitions, so reaching an `Ok`/`Err` at all proves the clash is gone.
        assert!(CountParent::try_parse_from(["prog", "auth", "status"]).is_ok());
        assert!(BoolParent::try_parse_from(["prog", "auth", "status"]).is_ok());

        // The parent's global `--verbose` is still accepted after the subcommand.
        assert!(CountParent::try_parse_from(["prog", "auth", "status", "--verbose"]).is_ok());
        // And our renamed detail flag works on its own.
        assert!(CountParent::try_parse_from(["prog", "auth", "status", "--details"]).is_ok());
    }

    #[test]
    fn token_metadata_extracts_expiry_scopes_and_claims() {
        let token = unsigned_jwt(json!({
            "exp": 1_900_000_000_i64,
            "scope": "channel:read channel:upload",
            "scp": ["openid", "profile"],
            "iss": "https://prefix.dev",
            "sub": "user-123",
            "aud": ["rattler", "prefix"]
        }));

        assert_eq!(
            token_metadata(&token),
            Some(TokenMetadata {
                expires_at: Some(1_900_000_000),
                scopes: vec![
                    "channel:read".to_string(),
                    "channel:upload".to_string(),
                    "openid".to_string(),
                    "profile".to_string()
                ],
                issuer: Some("https://prefix.dev".to_string()),
                subject: Some("user-123".to_string()),
                audience: vec!["rattler".to_string(), "prefix".to_string()],
            })
        );
    }

    #[tokio::test]
    async fn test_login_with_token_success() {
        let (storage, _temp_dir) = create_test_storage();

        // Mock the GraphQL API response
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/api/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("authorization", "Bearer valid_token")
            .with_body(
                json!({
                    "data": {
                        "viewer": {
                            "login": "testuser"
                        }
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create();

        let mut args = create_login_args("prefix.dev");
        args.token = Some("valid_token".to_string());

        // Use temp_env to isolate environment variable
        let result = async_with_vars(
            [("PREFIX_DEV_API_URL", Some(server.url().as_str()))],
            async { login(args, storage).await },
        )
        .await;

        assert!(result.is_ok());
        mock.assert();
    }

    #[tokio::test]
    async fn test_login_with_invalid_token() {
        let (storage, _temp_dir) = create_test_storage();

        // Mock the GraphQL API response for invalid token
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/api/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("authorization", "Bearer invalid_token")
            .with_body(
                json!({
                    "data": {
                        "viewer": null
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create();

        let mut args = create_login_args("prefix.dev");
        args.token = Some("invalid_token".to_string());

        // Use temp_env to isolate environment variable
        let result = async_with_vars(
            [("PREFIX_DEV_API_URL", Some(server.url().as_str()))],
            async { login(args, storage).await },
        )
        .await;

        // Now we expect an UnauthorizedToken error instead of Ok(())
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::UnauthorizedToken)
        ));

        mock.assert();
    }

    #[tokio::test]
    async fn test_login_missing_password_for_basic_auth() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("example.com");
        args.username = Some("testuser".to_string());
        // password I set here is:  None
        let result = login(args, storage).await;
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::MissingPassword)
        ));
    }

    #[tokio::test]
    async fn test_login_basic_auth_success() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("example.com");
        args.username = Some("testuser".to_string());
        args.password = Some("testpass".to_string());

        let result = login(args, storage).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_login_conda_token_success() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("anaconda.org");
        args.conda_token = Some("conda_token_123".to_string());

        let result = login(args, storage).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_login_s3_credentials_success() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("s3://my-bucket");
        args.s3_access_key_id = Some("access_key".to_string());
        args.s3_secret_access_key = Some("secret_key".to_string());
        args.s3_session_token = Some("session_token".to_string());

        let result = login(args, storage).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_login_no_authentication_method() {
        let (storage, _temp_dir) = create_test_storage();
        let args = create_login_args("example.com");
        // No authentication method provided

        let result = login(args, storage).await;
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::NoAuthenticationMethod)
        ));
    }

    #[tokio::test]
    async fn test_login_prefix_dev_requires_token() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("prefix.dev");
        args.username = Some("testuser".to_string());
        args.password = Some("testpass".to_string());

        let result = login(args, storage).await;
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::PrefixDevBadMethod)
        ));
    }

    #[tokio::test]
    async fn test_login_anaconda_org_requires_conda_token() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("anaconda.org");
        args.token = Some("bearer_token".to_string());

        let result = login(args, storage).await;
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::AnacondaOrgBadMethod)
        ));
    }

    #[tokio::test]
    async fn test_login_s3_requires_proper_credentials() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("s3://my-bucket");
        args.token = Some("bearer_token".to_string());

        let result = login(args, storage).await;
        assert!(matches!(result, Err(AuthenticationCLIError::S3BadMethod)));
    }

    #[tokio::test]
    async fn test_login_s3_credentials_with_non_s3_host() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("example.com");
        args.s3_access_key_id = Some("access_key".to_string());
        args.s3_secret_access_key = Some("secret_key".to_string());

        let result = login(args, storage).await;
        assert!(matches!(result, Err(AuthenticationCLIError::S3BadMethod)));
    }

    #[test]
    fn ensure_url_scheme_prepends_https_for_bare_host() {
        assert_eq!(ensure_url_scheme("prefix.dev"), "https://prefix.dev");
    }

    #[test]
    fn ensure_url_scheme_keeps_existing_https_scheme() {
        assert_eq!(
            ensure_url_scheme("https://prefix.dev"),
            "https://prefix.dev"
        );
    }

    #[test]
    fn ensure_url_scheme_keeps_existing_http_scheme() {
        assert_eq!(
            ensure_url_scheme("http://localhost:4444"),
            "http://localhost:4444"
        );
    }

    #[cfg(feature = "oauth")]
    #[test]
    fn test_default_oauth_config_for_host() {
        let has_default = |h: &str| default_oauth_config_for_host(h).is_some();

        assert!(has_default("prefix.dev"));
        assert!(has_default("repo.prefix.dev"));
        assert!(has_default("https://prefix.dev"));
        assert!(has_default("*.prefix.dev"));

        // Normalization: trailing slash and full URLs should still match.
        assert!(has_default("prefix.dev/"));
        assert!(has_default("https://prefix.dev/"));
        assert!(has_default("https://repo.prefix.dev/"));

        // Loopback addresses are not auto-recognized: local dev servers
        // could be running anything, so the user passes `--oauth` and
        // their own `--oauth-scope` flags explicitly.
        assert!(!has_default("localhost"));
        assert!(!has_default("localhost:8080"));
        assert!(!has_default("127.0.0.1"));

        assert!(!has_default("example.com"));
        // Suffix-injection guard: hostname containing "prefix.dev" must not match.
        assert!(!has_default("evil-prefix.dev.attacker.com"));
        assert!(!has_default("notprefix.dev"));

        // Returned config carries the right scheme + client_id for prefix.dev.
        let prefix = default_oauth_config_for_host("prefix.dev").unwrap();
        assert_eq!(prefix.issuer_url, "https://prefix.dev");
        assert_eq!(prefix.client_id, "rattler");
        assert!(prefix.scopes.iter().any(|s| s == "channel:upload"));
    }

    #[cfg(feature = "oauth")]
    #[test]
    fn test_default_oauth_for_login() {
        // No explicit method on prefix.dev → OAuth default kicks in
        assert!(default_oauth_for_login(&create_login_args("prefix.dev")).is_some());

        // Explicit method blocks the OAuth default, even on prefix.dev.
        let mut args = create_login_args("prefix.dev");
        args.token = Some("t".into());
        assert!(default_oauth_for_login(&args).is_none());

        // No explicit method on a non-OAuth host → still falls through to existing
        // NoAuthenticationMethod error.
        assert!(default_oauth_for_login(&create_login_args("example.com")).is_none());
    }

    fn logout_args(host: Option<&str>, all: bool) -> LogoutArgs {
        LogoutArgs {
            host: host.map(ToString::to_string),
            all,
        }
    }

    #[test]
    fn logout_candidate_keys_covers_both_login_canonical_forms() {
        // For top-level-domain-style hosts, both forms must appear so we hit
        // legacy entries stored by either login path.
        let keys = logout_candidate_keys("prefix.dev").unwrap();
        assert!(keys.contains(&"prefix.dev".to_string()));
        assert!(keys.contains(&"*.prefix.dev".to_string()));

        // Full URL inputs normalize down to the same candidates.
        let keys = logout_candidate_keys("https://prefix.dev/").unwrap();
        assert!(keys.contains(&"prefix.dev".to_string()));
        assert!(keys.contains(&"*.prefix.dev".to_string()));
    }

    #[tokio::test]
    async fn logout_removes_legacy_wildcard_entry() {
        // Simulates a token/basic login on a TLD-style host: stored under
        // `*.prefix.dev` by `get_url`.
        let (storage, _temp_dir) = create_test_storage();
        storage
            .store("*.prefix.dev", &Authentication::BearerToken("tok".into()))
            .unwrap();

        logout(logout_args(Some("prefix.dev"), false), storage.clone())
            .await
            .unwrap();

        assert!(storage.list().unwrap().is_empty());
    }

    #[cfg(feature = "oauth")]
    #[tokio::test]
    async fn logout_removes_legacy_oauth_entry_without_wildcard() {
        // Simulates an OAuth login: stored under `prefix.dev` (no wildcard)
        // by `normalize_login_host`. The old logout used `get_url` and would
        // never find this entry.
        let (storage, _temp_dir) = create_test_storage();
        storage
            .store(
                "prefix.dev",
                &Authentication::OAuth {
                    access_token: "a".into(),
                    refresh_token: None,
                    expires_at: None,
                    token_endpoint: "https://prefix.dev/token".into(),
                    // No revocation endpoint → revoke_tokens won't run, so
                    // this test stays hermetic (no HTTP needed).
                    revocation_endpoint: None,
                    client_id: "rattler".into(),
                },
            )
            .unwrap();

        logout(logout_args(Some("prefix.dev"), false), storage.clone())
            .await
            .unwrap();

        assert!(storage.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn logout_unknown_host_returns_not_logged_in() {
        let (storage, _temp_dir) = create_test_storage();
        let result = logout(logout_args(Some("nothing-here.example"), false), storage).await;
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::NotLoggedIn(_))
        ));
    }

    #[tokio::test]
    async fn logout_all_clears_every_entry() {
        let (storage, _temp_dir) = create_test_storage();
        storage
            .store("*.prefix.dev", &Authentication::BearerToken("t".into()))
            .unwrap();
        storage
            .store("*.anaconda.org", &Authentication::CondaToken("c".into()))
            .unwrap();

        logout(logout_args(None, true), storage.clone())
            .await
            .unwrap();

        assert!(storage.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn logout_all_on_empty_storage_is_a_noop() {
        let (storage, _temp_dir) = create_test_storage();
        // `--all` on a fresh machine should succeed silently, not error.
        logout(logout_args(None, true), storage).await.unwrap();
    }

    /// `delete_entry` must remove the host from ONLY the selected backend,
    /// leaving shadowed copies in other backends intact. This is what makes
    /// the interactive picker work correctly: picking one entry doesn't wipe
    /// every backend's copy.
    #[tokio::test]
    async fn delete_entry_only_touches_the_named_backend() {
        use rattler_networking::authentication_storage::StorageBackend;
        let mut storage = AuthenticationStorage::empty();
        let backend_a = std::sync::Arc::new(MemoryStorage::with_name("a"));
        let backend_b = std::sync::Arc::new(MemoryStorage::with_name("b"));
        storage.add_backend(backend_a.clone());
        storage.add_backend(backend_b.clone());

        backend_a
            .store("prefix.dev", &Authentication::BearerToken("tok-a".into()))
            .unwrap();
        backend_b
            .store("prefix.dev", &Authentication::BearerToken("tok-b".into()))
            .unwrap();

        // Find the listed entry for backend B and ask storage to delete just it.
        let entries = storage.list_with_sources().unwrap();
        let b_entry = entries
            .iter()
            .find(|e| e.source == backend_b.name())
            .expect("backend B's entry should be listed");
        storage
            .delete_entry(&b_entry.host, &b_entry.source)
            .unwrap();

        // Backend B is empty…
        assert!(
            backend_b.list().unwrap().is_empty(),
            "selected backend should be cleared"
        );
        // …but backend A still holds its copy.
        assert_eq!(
            backend_a.list().unwrap(),
            vec![(
                "prefix.dev".to_string(),
                Authentication::BearerToken("tok-a".into())
            )],
            "unselected backend must keep its copy"
        );
    }

    /// When the same host has OAuth credentials in multiple backends (i.e.
    /// one entry is "active" and the rest are "shadowed"), logout must
    /// revoke every copy's tokens at the `IdP` — not just the active one —
    /// and clear all backends.
    #[cfg(feature = "oauth")]
    #[tokio::test]
    async fn logout_revokes_oauth_tokens_in_every_backend() {
        use rattler_networking::authentication_storage::StorageBackend;
        let mut server = Server::new_async().await;
        // Two backends × one access-token revoke call each = 2 hits.
        let revoke_mock = server
            .mock("POST", "/revoke")
            .expect(2)
            .with_status(200)
            .create_async()
            .await;

        let mut storage = AuthenticationStorage::empty();
        // Distinct names so `delete_entry` can identify each backend
        // unambiguously (production backends always have unique identifiers
        // — file paths, keyring type, etc.).
        let backend_a = std::sync::Arc::new(MemoryStorage::with_name("a"));
        let backend_b = std::sync::Arc::new(MemoryStorage::with_name("b"));
        storage.add_backend(backend_a.clone());
        storage.add_backend(backend_b.clone());

        let oauth = Authentication::OAuth {
            access_token: "tok".into(),
            refresh_token: None,
            expires_at: None,
            token_endpoint: format!("{}/token", server.url()),
            revocation_endpoint: Some(format!("{}/revoke", server.url())),
            client_id: "rattler".into(),
        };
        // Write directly to each backend so both end up holding the entry —
        // `storage.store()` stops at the first successful backend.
        backend_a.store("prefix.dev", &oauth).unwrap();
        backend_b.store("prefix.dev", &oauth).unwrap();

        logout(logout_args(Some("prefix.dev"), false), storage.clone())
            .await
            .unwrap();

        revoke_mock.assert_async().await;
        assert!(
            storage.list_with_sources().unwrap().is_empty(),
            "both backends should be cleared, including the shadowed copy"
        );
    }

    #[test]
    fn logout_args_reject_host_combined_with_all() {
        assert!(LogoutArgs::try_parse_from(["logout", "prefix.dev", "--all"]).is_err());
    }

    /// Without the interactive picker, a bare `auth logout` has nothing
    /// sensible to do — clap must reject it instead of silently no-opping.
    #[cfg(not(feature = "auth-interactive"))]
    #[test]
    fn logout_args_require_host_or_all_without_interactive() {
        assert!(LogoutArgs::try_parse_from(["logout"]).is_err());
    }

    #[cfg(feature = "auth-interactive")]
    #[test]
    fn logout_args_allow_bare_invocation_with_interactive() {
        assert!(LogoutArgs::try_parse_from(["logout"]).is_ok());
    }

    /// A bare interactive logout must fail fast off a TTY instead of
    /// handing a picker to a terminal that isn't there.
    #[cfg(feature = "auth-interactive")]
    #[tokio::test]
    async fn interactive_logout_without_tty_errors() {
        use std::io::IsTerminal;

        // Only meaningful where stdin/stdout aren't TTYs (CI, nextest). In an
        // interactive `cargo test` run this would open a real picker, so skip.
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return;
        }

        let (storage, _temp_dir) = create_test_storage();
        storage
            .store("example.com", &Authentication::BearerToken("tok".into()))
            .unwrap();

        let result = logout(logout_args(None, false), storage).await;
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::NotInteractive)
        ));
    }
}
