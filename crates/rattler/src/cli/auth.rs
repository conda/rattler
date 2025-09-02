//! This module contains CLI common entrypoint for authentication.
use clap::Parser;
use rattler_networking::{
    authentication_storage::AuthenticationStorageError, Authentication, AuthenticationStorage,
};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::json;
use thiserror;

/// Command line arguments that contain authentication data
#[derive(Parser, Debug)]
struct LoginArgs {
    /// The host to authenticate with (e.g. prefix.dev)
    host: String,

    /// The token to use (for authentication with prefix.dev)
    #[clap(long)]
    token: Option<String>,

    /// The username to use (for basic HTTP authentication)
    #[clap(long)]
    username: Option<String>,

    /// The password to use (for basic HTTP authentication)
    #[clap(long)]
    password: Option<String>,

    /// The token to use on anaconda.org / quetz authentication
    #[clap(long)]
    conda_token: Option<String>,

    /// The S3 access key ID
    #[clap(long, requires_all = ["s3_secret_access_key"], conflicts_with_all = ["token", "username", "password", "conda_token"])]
    s3_access_key_id: Option<String>,

    /// The S3 secret access key
    #[clap(long, requires_all = ["s3_access_key_id"])]
    s3_secret_access_key: Option<String>,

    /// The S3 session token
    #[clap(long, requires_all = ["s3_access_key_id"])]
    s3_session_token: Option<String>,
}

#[derive(Parser, Debug)]
struct LogoutArgs {
    /// The host to remove authentication for
    host: String,
}

#[derive(Parser, Debug)]
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
    #[error("Authentication with anaconda.org requires a conda token. Use `--conda-token` to provide one")]
    AnacondaOrgBadMethod,

    /// Bad authentication method when using S3
    #[error("Authentication with S3 requires a S3 access key ID and a secret access key. Use `--s3-access-key-id` and `--s3-secret-access-key` to provide them")]
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
    Valid(String),
    /// Token is invalid or unauthorized
    Invalid,
}

/// Authenticate with a host using the provided credentials.
///
/// This function validates the authentication method based on the host and stores
/// the credentials if successful. For prefix.dev hosts, it validates the token
/// by making a GraphQL API call.
///
fn login(args: LoginArgs, storage: AuthenticationStorage) -> Result<(), AuthenticationCLIError> {
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
        match validate_prefix_dev_token(token, &args.host)? {
            ValidationResult::Valid(username) => {
                println!("✅ Token is valid. Logged in as \"{username}\". Storing credentials...");
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
fn validate_prefix_dev_token(
    token: &str,
    host: &str,
) -> Result<ValidationResult, AuthenticationCLIError> {
    // Validate whether the user exists
    let client = Client::new();

    // Allow override of API URL for testing
    let mut prefix_url = if host.contains("://") {
        host.to_string()
    } else {
        format!("https://{host}")
    };

    if prefix_url.contains("*.") {
        // strip wildcard if given
        prefix_url = prefix_url.replace("*.", "");
    }

    if let Ok(env_var) = std::env::var("PREFIX_DEV_API_URL") {
        prefix_url = env_var;
    }

    let body = json!({
        "query": "query { viewer { login } }"
    });

    let response = client
        .post(prefix_url)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(CONTENT_TYPE, "application/json")
        .json(&body)
        .send()?;

    let text = response.text()?;

    // Parse JSON
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| AuthenticationCLIError::JsonParseError(e.to_string()))?;

    // Check if viewer is null (invalid token) or contains user data (valid token)
    match &json["data"]["viewer"] {
        serde_json::Value::Null => Ok(ValidationResult::Invalid),
        viewer_data => {
            if let Some(username) = viewer_data["login"].as_str() {
                Ok(ValidationResult::Valid(username.to_string()))
            } else {
                Ok(ValidationResult::Invalid)
            }
        }
    }
}

fn logout(args: LogoutArgs, storage: AuthenticationStorage) -> Result<(), AuthenticationCLIError> {
    let host = get_url(&args.host)?;

    println!("Removing authentication for {host}");

    storage.delete(&host)?;
    Ok(())
}

/// CLI entrypoint for authentication
pub async fn execute(args: Args) -> Result<(), AuthenticationCLIError> {
    let storage = AuthenticationStorage::from_env_and_defaults()?;

    match args.subcommand {
        Subcommand::Login(args) => login(args, storage),
        Subcommand::Logout(args) => logout(args, storage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use rattler_networking::AuthenticationStorage;
    use serde_json::json;
    use tempfile::TempDir;

    // Helper function to create a test authentication storage
    fn create_test_storage() -> (AuthenticationStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = AuthenticationStorage::from_env_and_defaults().unwrap();
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
        }
    }

    #[test]
    fn test_login_with_token_success() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("prefix.dev");
        args.token = Some("valid_token".to_string());

        // Mock the GraphQL API response
        let mut server = Server::new();
        let mock = server
            .mock("POST", "/api/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
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
            .create();

        // Set environment variable to point to mock server
        std::env::set_var(
            "PREFIX_DEV_API_URL",
            format!("{}/api/graphql", server.url()),
        );
        let result = login(args, storage);
        assert!(result.is_ok());
        mock.assert();
        // Clean up
        std::env::remove_var("PREFIX_DEV_API_URL");
    }

    #[test]
    fn test_login_with_invalid_token() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("prefix.dev");
        args.token = Some("invalid_token".to_string());

        // Mock the GraphQL API response for invalid token
        let mut server = Server::new();
        let mock = server
            .mock("POST", "/api/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "data": {
                        "viewer": null
                    }
                })
                .to_string(),
            )
            .create();

        // Set environment variable to point to mock server
        std::env::set_var(
            "PREFIX_DEV_API_URL",
            format!("{}/api/graphql", server.url()),
        );

        let result = login(args, storage);

        // Now we expect an UnauthorizedToken error instead of Ok(())
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::UnauthorizedToken)
        ));

        mock.assert();

        // Clean up
        std::env::remove_var("PREFIX_DEV_API_URL");
    }

    #[test]
    fn test_login_missing_password_for_basic_auth() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("example.com");
        args.username = Some("testuser".to_string());
        // password I set herer is:  None
        let result = login(args, storage);
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::MissingPassword)
        ));
    }

    #[test]
    fn test_login_basic_auth_success() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("example.com");
        args.username = Some("testuser".to_string());
        args.password = Some("testpass".to_string());

        let result = login(args, storage);
        assert!(result.is_ok());
    }

    #[test]
    fn test_login_conda_token_success() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("anaconda.org");
        args.conda_token = Some("conda_token_123".to_string());

        let result = login(args, storage);
        assert!(result.is_ok());
    }

    #[test]
    fn test_login_s3_credentials_success() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("s3://my-bucket");
        args.s3_access_key_id = Some("access_key".to_string());
        args.s3_secret_access_key = Some("secret_key".to_string());
        args.s3_session_token = Some("session_token".to_string());

        let result = login(args, storage);
        assert!(result.is_ok());
    }

    #[test]
    fn test_login_no_authentication_method() {
        let (storage, _temp_dir) = create_test_storage();
        let args = create_login_args("example.com");
        // No authentication method provided

        let result = login(args, storage);
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::NoAuthenticationMethod)
        ));
    }

    #[test]
    fn test_login_prefix_dev_requires_token() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("prefix.dev");
        args.username = Some("testuser".to_string());
        args.password = Some("testpass".to_string());

        let result = login(args, storage);
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::PrefixDevBadMethod)
        ));
    }

    #[test]
    fn test_login_anaconda_org_requires_conda_token() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("anaconda.org");
        args.token = Some("bearer_token".to_string());

        let result = login(args, storage);
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::AnacondaOrgBadMethod)
        ));
    }

    #[test]
    fn test_login_s3_requires_proper_credentials() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("s3://my-bucket");
        args.token = Some("bearer_token".to_string());

        let result = login(args, storage);
        assert!(matches!(result, Err(AuthenticationCLIError::S3BadMethod)));
    }

    #[test]
    fn test_login_s3_credentials_with_non_s3_host() {
        let (storage, _temp_dir) = create_test_storage();
        let mut args = create_login_args("example.com");
        args.s3_access_key_id = Some("access_key".to_string());
        args.s3_secret_access_key = Some("secret_key".to_string());

        let result = login(args, storage);
        assert!(matches!(result, Err(AuthenticationCLIError::S3BadMethod)));
    }
}
