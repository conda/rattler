//! This module contains CLI common entrypoint for authentication.

#[cfg(feature = "oauth")]
pub mod oauth;

use clap::Parser;
use rattler_networking::{
    authentication_storage::AuthenticationStorageError, Authentication, AuthenticationStorage,
};
use reqwest::{header::CONTENT_TYPE, Client};
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
        let issuer_url = args
            .oauth_issuer_url
            .unwrap_or_else(|| format!("https://{}", args.host));
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
        // OAuth credentials are issuer-specific, skip wildcard conversion
        let host = args.host.clone();
        storage.store(&host, &auth)?;
        eprintln!("Credentials stored for {host}.");
        return Ok(());
    }

    let auth = if let Some(conda_token) = args.conda_token {
        Authentication::CondaToken(conda_token)
    } else if let Some(username) = args.username {
        if args.password.is_none() {
            return Err(AuthenticationCLIError::MissingPassword);
        } else {
            let password = args.password.unwrap();
            Authentication::BasicHTTP { username, password }
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
        // password I set herer is:  None
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
}
