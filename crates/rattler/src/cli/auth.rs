//! This module contains CLI common entrypoint for authentication.
use clap::Parser;
use rattler_networking::{Authentication, AuthenticationStorage};
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

    /// Wrapper for errors that are generated from the underlying storage system
    /// (keyring or file system)
    #[error("Failed to interact with the authentication storage system")]
    StorageError(#[source] anyhow::Error),
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
    let host = get_url(&args.host)?;
    println!("Authenticating with {host}");

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
    } else {
        return Err(AuthenticationCLIError::NoAuthenticationMethod);
    };

    if host.contains("prefix.dev") && !matches!(auth, Authentication::BearerToken(_)) {
        return Err(AuthenticationCLIError::PrefixDevBadMethod);
    }

    if host.contains("anaconda.org") && !matches!(auth, Authentication::CondaToken(_)) {
        return Err(AuthenticationCLIError::AnacondaOrgBadMethod);
    }

    storage
        .store(&host, &auth)
        .map_err(AuthenticationCLIError::StorageError)?;
    Ok(())
}

fn logout(args: LogoutArgs, storage: AuthenticationStorage) -> Result<(), AuthenticationCLIError> {
    let host = get_url(&args.host)?;

    println!("Removing authentication for {host}");

    storage
        .delete(&host)
        .map_err(AuthenticationCLIError::StorageError)?;
    Ok(())
}

/// CLI entrypoint for authentication
pub async fn execute(args: Args) -> Result<(), AuthenticationCLIError> {
    let storage = AuthenticationStorage::default();

    match args.subcommand {
        Subcommand::Login(args) => login(args, storage),
        Subcommand::Logout(args) => logout(args, storage),
    }
}
