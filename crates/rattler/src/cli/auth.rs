//! This module contains CLI common entrypoint for authentication.

#[cfg(feature = "oauth")]
pub mod oauth;

use clap::Parser;
use rattler_networking::{
    authentication_storage::AuthenticationStorageError, Authentication, AuthenticationStorage,
};
use reqwest::{header::CONTENT_TYPE, Client};
#[cfg(feature = "oauth")]
use serde::Deserialize;
use serde_json::json;
use thiserror;
use url::Url;

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

    /// OAuth flow: auto (default), auth-code, device-code
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", value_parser = ["auto", "auth-code", "device-code"], help_heading = "OAuth/OIDC Authentication")]
    oauth_flow: Option<String>,

    /// Additional OAuth scopes to request (repeatable)
    #[cfg(feature = "oauth")]
    #[clap(
        long = "oauth-scope",
        requires = "oauth",
        help_heading = "OAuth/OIDC Authentication"
    )]
    oauth_scopes: Vec<String>,

    /// Use Artifactory machine-to-machine login with GitHub Actions OIDC
    #[cfg(feature = "oauth")]
    #[clap(long, requires_all = ["oauth", "artifactory_provider_name"], help_heading = "Artifactory OIDC")]
    artifactory_m2m: bool,

    /// Artifactory OIDC provider name (matches JFrog OIDC integration)
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", help_heading = "Artifactory OIDC")]
    artifactory_provider_name: Option<String>,

    /// Audience to request from GitHub Actions OIDC endpoint
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", help_heading = "Artifactory OIDC")]
    artifactory_audience: Option<String>,

    /// Optional Artifactory registry URL for login target normalization
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", help_heading = "Artifactory OIDC")]
    artifactory_registry_url: Option<String>,

    /// Repository type used when normalizing shorthand Artifactory registry URLs
    #[cfg(feature = "oauth")]
    #[clap(long, requires = "oauth", value_parser = ["npm", "conda", "pypi", "maven", "nuget", "generic"], help_heading = "Artifactory OIDC")]
    artifactory_repository_type: Option<String>,
}

#[derive(Parser, Debug)]
struct LogoutArgs {
    /// The host to remove authentication for
    host: String,
}

#[derive(Parser, Debug)]
#[allow(clippy::large_enum_variant)]
enum Subcommand {
    /// Store authentication information for a given host
    Login(LoginArgs),
    /// Remove authentication information for a given host
    Logout(LogoutArgs),
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

    /// JSON parsing failed
    #[error("Failed to parse JSON: {0}")]
    JsonParseError(String),

    /// Token is unauthorized or invalid
    #[error("Unauthorized or invalid token")]
    UnauthorizedToken,

    /// OAuth error
    #[cfg(feature = "oauth")]
    #[error(transparent)]
    OAuthError(#[from] oauth::OAuthError),

    /// A required environment variable for GitHub OIDC was not found.
    #[cfg(feature = "oauth")]
    #[error("Environment variable {0} not set. For GitHub Actions OIDC ensure `permissions: id-token: write` is configured")]
    MissingGitHubOidcEnvVar(&'static str),

    /// Artifactory OIDC token exchange failed.
    #[cfg(feature = "oauth")]
    #[error("Artifactory OIDC token exchange failed: {0}")]
    ArtifactoryOidcExchange(String),
}

#[cfg(feature = "oauth")]
const ACTIONS_ID_TOKEN_REQUEST_URL: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";
#[cfg(feature = "oauth")]
const ACTIONS_ID_TOKEN_REQUEST_TOKEN: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";

#[cfg(feature = "oauth")]
#[derive(Debug, Deserialize)]
struct GitHubOidcTokenResponse {
    value: String,
}

#[cfg(feature = "oauth")]
#[derive(Debug, Deserialize)]
struct ArtifactoryOidcTokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[cfg(feature = "oauth")]
fn normalize_storage_host(input: &str) -> Result<String, AuthenticationCLIError> {
    if input.contains("://") {
        let parsed = Url::parse(input)?;
        if let Some(host) = parsed.host_str() {
            return Ok(host.to_string());
        }
    }
    Ok(input.to_string())
}

#[cfg(feature = "oauth")]
fn parse_url_or_host(input: &str) -> Result<Url, AuthenticationCLIError> {
    if input.contains("://") {
        return Url::parse(input).map_err(AuthenticationCLIError::from);
    }
    Url::parse(&format!("https://{input}")).map_err(AuthenticationCLIError::from)
}

#[cfg(feature = "oauth")]
fn derive_artifactory_base_url(input: &str) -> Result<Url, AuthenticationCLIError> {
    let mut url = parse_url_or_host(input)?;

    if let Some(index) = url.path().find("/artifactory") {
        url.set_path(&url.path()[..index]);
    } else {
        url.set_path("");
    }

    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

#[cfg(feature = "oauth")]
fn normalize_artifactory_registry_url(
    input: &str,
    repo_type: Option<&str>,
) -> Result<Url, AuthenticationCLIError> {
    let mut url = parse_url_or_host(input)?;
    let path = url.path().trim_matches('/');
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };

    if segments.len() >= 4
        && segments[0] == "artifactory"
        && segments[1] == "api"
        && !segments[2].is_empty()
        && !segments[3].is_empty()
    {
        url.set_path(&format!(
            "/artifactory/api/{}/{}/",
            segments[2], segments[3]
        ));
        return Ok(url);
    }

    if segments.len() == 2 && segments[0] == "artifactory" {
        let normalized_type = repo_type.unwrap_or("npm");
        url.set_path(&format!(
            "/artifactory/api/{}/{}/",
            normalized_type, segments[1]
        ));
        return Ok(url);
    }

    let known_repository_types = ["npm", "conda", "pypi", "maven", "nuget", "generic"];
    if segments.len() == 3
        && segments[0] == "artifactory"
        && known_repository_types.contains(&segments[1])
    {
        url.set_path(&format!(
            "/artifactory/api/{}/{}/",
            segments[1], segments[2]
        ));
        return Ok(url);
    }

    Ok(url)
}

#[cfg(feature = "oauth")]
async fn get_github_actions_oidc_token(
    audience: &str,
) -> Result<String, AuthenticationCLIError> {
    let request_url = std::env::var(ACTIONS_ID_TOKEN_REQUEST_URL)
        .map_err(|_| AuthenticationCLIError::MissingGitHubOidcEnvVar(ACTIONS_ID_TOKEN_REQUEST_URL))?;
    let request_token = std::env::var(ACTIONS_ID_TOKEN_REQUEST_TOKEN).map_err(|_| {
        AuthenticationCLIError::MissingGitHubOidcEnvVar(ACTIONS_ID_TOKEN_REQUEST_TOKEN)
    })?;

    let mut url = Url::parse(&request_url)?;
    url.query_pairs_mut().append_pair("audience", audience);

    let response = Client::new()
        .get(url)
        .bearer_auth(request_token)
        .send()
        .await?
        .error_for_status()?;

    let body: GitHubOidcTokenResponse = response.json().await?;
    Ok(body.value)
}

#[cfg(feature = "oauth")]
async fn exchange_artifactory_oidc_token(
    artifactory_base_url: &Url,
    provider_name: &str,
    id_token: &str,
) -> Result<ArtifactoryOidcTokenResponse, AuthenticationCLIError> {
    let exchange_url = artifactory_base_url.join("access/api/v1/oidc/token")?;

    let payload = json!({
        "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
        "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
        "subject_token": id_token,
        "provider_name": provider_name,
        "project_key": std::env::var("JF_PROJECT").unwrap_or_default(),
        "gh_job_id": std::env::var("GITHUB_JOB").unwrap_or_default(),
        "gh_run_id": std::env::var("GITHUB_RUN_ID").unwrap_or_default(),
        "gh_repo": std::env::var("GITHUB_REPOSITORY").unwrap_or_default(),
        "gh_revision": std::env::var("GITHUB_SHA").unwrap_or_default(),
        "gh_branch": std::env::var("GITHUB_REF_NAME").unwrap_or_default(),
        "repo": std::env::var("GITHUB_REPOSITORY").ok(),
        "revision": std::env::var("GITHUB_SHA").ok(),
        "branch": std::env::var("GITHUB_REF_NAME").ok(),
    });

    let response = Client::new()
        .post(exchange_url.clone())
        .header(CONTENT_TYPE, "application/json")
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(AuthenticationCLIError::ArtifactoryOidcExchange(format!(
            "HTTP {}: {}",
            status,
            body
        )));
    }

    serde_json::from_str::<ArtifactoryOidcTokenResponse>(&body).map_err(|e| {
        AuthenticationCLIError::ArtifactoryOidcExchange(format!(
            "failed to parse exchange response: {e}; body: {body}"
        ))
    })
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
async fn login(
    args: LoginArgs,
    storage: AuthenticationStorage,
) -> Result<(), AuthenticationCLIError> {
    // OAuth flow (when --oauth is set)
    #[cfg(feature = "oauth")]
    if args.oauth {
        let normalized_registry_url = if let Some(registry_url) = args.artifactory_registry_url.as_deref() {
            let normalized = normalize_artifactory_registry_url(
                registry_url,
                args.artifactory_repository_type.as_deref(),
            )?;
            eprintln!("Using Artifactory registry URL: {normalized}");
            Some(normalized)
        } else {
            None
        };

        if args.artifactory_m2m {
            let provider_name = args.artifactory_provider_name.as_deref().ok_or(
                AuthenticationCLIError::ArtifactoryOidcExchange(
                    "--artifactory-provider-name is required for --artifactory-m2m".to_string(),
                ),
            )?;

            let audience = args.artifactory_audience.as_deref().unwrap_or("api://default");

            let target_for_base = normalized_registry_url
                .as_ref()
                .map(Url::as_str)
                .unwrap_or(args.host.as_str());
            let base_url = derive_artifactory_base_url(target_for_base)?;

            let id_token = get_github_actions_oidc_token(audience).await?;
            let exchanged = exchange_artifactory_oidc_token(&base_url, provider_name, &id_token).await?;

            let expires_at = exchanged.expires_in.map(|seconds| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
                    + seconds
            });

            let auth = Authentication::OAuth {
                access_token: exchanged.access_token,
                refresh_token: exchanged.refresh_token,
                expires_at,
                token_endpoint: base_url.join("access/api/v1/oidc/token")?.to_string(),
                revocation_endpoint: None,
                client_id: provider_name.to_string(),
            };

            let storage_key = if let Some(registry) = normalized_registry_url {
                registry.host_str().unwrap_or(args.host.as_str()).to_string()
            } else {
                normalize_storage_host(&args.host)?
            };

            storage.store(&storage_key, &auth)?;
            eprintln!(
                "Artifactory OIDC credentials stored for {storage_key} (machine-to-machine)."
            );
            return Ok(());
        }

        let default_issuer = if args.artifactory_provider_name.is_some()
            || args.artifactory_registry_url.is_some()
        {
            let target_for_base = normalized_registry_url
                .as_ref()
                .map(Url::as_str)
                .unwrap_or(args.host.as_str());
            let base_url = derive_artifactory_base_url(target_for_base)?;
            base_url.join("access")?.to_string()
        } else {
            format!("https://{}", args.host)
        };

        let issuer_url = args.oauth_issuer_url.unwrap_or(default_issuer);
        let client_id = args
            .oauth_client_id
            .unwrap_or_else(|| "rattler".to_string());
        let flow = match args.oauth_flow.as_deref() {
            Some("auth-code") => oauth::OAuthFlow::AuthCode,
            Some("device-code") => oauth::OAuthFlow::DeviceCode,
            _ => oauth::OAuthFlow::Auto,
        };

        let config = oauth::OAuthConfig {
            issuer_url,
            client_id,
            client_secret: args.oauth_client_secret,
            flow,
            scopes: args.oauth_scopes.into_iter().collect(),
        };

        let auth = oauth::perform_oauth_login(config).await?;
        let host = if let Some(registry) = normalized_registry_url {
            registry.host_str().unwrap_or(args.host.as_str()).to_string()
        } else {
            normalize_storage_host(&args.host)?
        };
        storage.store(&host, &auth)?;
        eprintln!("Credentials stored for {host}.");
        return Ok(());
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
        // Extract the token from BearerToken
        let token = match &auth {
            Authentication::BearerToken(t) => t,
            _ => return Err(AuthenticationCLIError::PrefixDevBadMethod),
        };

        // Validate the token using the extracted function
        match validate_prefix_dev_token(token, &args.host).await? {
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
) -> Result<ValidationResult, AuthenticationCLIError> {
    let prefix_url = if let Ok(env_var) = std::env::var("PREFIX_DEV_API_URL") {
        // If env var is set, parse it as a full URL
        Url::parse(&env_var).expect("PREFIX_DEV_API_URL must be a valid URL")
    } else {
        // Strip wildcard if given
        let host = host.replace("*.", "");

        // Convert the host URL to a full URL if it doesn't contain a scheme
        let host_url = if host.contains("://") {
            Url::parse(&host)?
        } else {
            Url::parse(&format!("https://{host}"))?
        };

        let host_url = host_url.host_str().unwrap_or("prefix.dev");
        // Strip "repo." prefix if present
        let host_url = host_url.strip_prefix("repo.").unwrap_or(host_url);

        Url::parse(&format!("https://{host_url}")).expect("constructed url must be valid")
    };

    let body = json!({
        "query": "query { viewer { login } }"
    });

    let client = Client::new();
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

async fn logout(
    args: LogoutArgs,
    storage: AuthenticationStorage,
) -> Result<(), AuthenticationCLIError> {
    let host = get_url(&args.host)?;

    // Revoke OAuth tokens before deleting credentials
    #[cfg(feature = "oauth")]
    if let Ok(Some(Authentication::OAuth {
        ref access_token,
        ref refresh_token,
        revocation_endpoint: Some(ref revocation_endpoint),
        ref client_id,
        ..
    })) = storage.get(&host)
    {
        eprintln!("Revoking OAuth tokens...");
        oauth::revoke_tokens(
            revocation_endpoint,
            access_token,
            refresh_token.as_deref(),
            client_id,
        )
        .await;
    }

    println!("Removing authentication for {host}");

    storage.delete(&host)?;
    Ok(())
}

/// CLI entrypoint for authentication
pub async fn execute(args: Args) -> Result<(), AuthenticationCLIError> {
    let storage = AuthenticationStorage::from_env_and_defaults()?;

    match args.subcommand {
        Subcommand::Login(args) => login(args, storage).await,
        Subcommand::Logout(args) => logout(args, storage).await,
    }
}

#[cfg(test)]
mod tests {
    use mockito::Server;
    use rattler_networking::{
        authentication_storage::backends::memory::MemoryStorage, AuthenticationStorage,
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
            artifactory_m2m: false,
            #[cfg(feature = "oauth")]
            artifactory_provider_name: None,
            #[cfg(feature = "oauth")]
            artifactory_audience: None,
            #[cfg(feature = "oauth")]
            artifactory_registry_url: None,
            #[cfg(feature = "oauth")]
            artifactory_repository_type: None,
        }
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

    #[cfg(feature = "oauth")]
    #[test]
    fn test_normalize_artifactory_registry_url_from_shorthand_repo() {
        let normalized = normalize_artifactory_registry_url(
            "https://my-org.jfrog.io/artifactory/npmtest-npm",
            Some("npm"),
        )
        .unwrap();
        assert_eq!(
            normalized.as_str(),
            "https://my-org.jfrog.io/artifactory/api/npm/npmtest-npm/"
        );
    }

    #[cfg(feature = "oauth")]
    #[test]
    fn test_normalize_artifactory_registry_url_from_kind_and_repo() {
        let normalized = normalize_artifactory_registry_url(
            "https://my-org.jfrog.io/artifactory/conda/conda-local",
            None,
        )
        .unwrap();
        assert_eq!(
            normalized.as_str(),
            "https://my-org.jfrog.io/artifactory/api/conda/conda-local/"
        );
    }

    #[cfg(feature = "oauth")]
    #[test]
    fn test_normalize_storage_host_from_url() {
        let host = normalize_storage_host("https://my-org.jfrog.io/artifactory/api/npm/repo/")
            .unwrap();
        assert_eq!(host, "my-org.jfrog.io");
    }

    #[cfg(feature = "oauth")]
    #[tokio::test]
    async fn test_get_github_actions_oidc_token() {
        let mut server = Server::new_async().await;
        let oidc_url = format!("{}/oidc", server.url());
        let mock = server
            .mock("GET", "/oidc")
            .match_query(mockito::Matcher::UrlEncoded(
                "audience".into(),
                "jfrog-github".into(),
            ))
            .match_header("authorization", "Bearer request-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"value":"jwt-token"}"#)
            .create();

        let result = async_with_vars(
            [
                (
                    ACTIONS_ID_TOKEN_REQUEST_URL,
                    Some(oidc_url.as_str()),
                ),
                (ACTIONS_ID_TOKEN_REQUEST_TOKEN, Some("request-token")),
            ],
            async { get_github_actions_oidc_token("jfrog-github").await },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "jwt-token");
        mock.assert();
    }

    #[cfg(feature = "oauth")]
    #[tokio::test]
    async fn test_exchange_artifactory_oidc_token() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/access/api/v1/oidc/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"art-token","expires_in":3600}"#)
            .create();

        let base_url = Url::parse(&server.url()).unwrap();
        let result = exchange_artifactory_oidc_token(&base_url, "my-provider", "id-token").await;

        assert!(result.is_ok());
        let token = result.unwrap();
        assert_eq!(token.access_token, "art-token");
        assert_eq!(token.expires_in, Some(3600));
        mock.assert();
    }
}
