//! This module contains CLI common entrypoint for authentication.
use clap::Parser;
use rattler_networking::{
    authentication_storage::AuthenticationStorageError, Authentication, AuthenticationStorage,
};
use thiserror;

/// Command line arguments that contain authentication data
#[derive(Parser, Debug)]
pub struct LoginArgs {
    /// The host to authenticate with (e.g. repo.prefix.dev)
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

    storage.store(&host, &auth)?;
    Ok(())
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
