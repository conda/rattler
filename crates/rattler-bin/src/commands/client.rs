use std::{collections::HashMap, sync::Arc};

use miette::{Context, IntoDiagnostic};
use rattler_networking::{
    AuthChallengeMiddleware, AuthenticationMiddleware, AuthenticationStorage, OfflineMiddleware,
};
use rattler_repodata_gateway::fetch::CacheAction;
use reqwest::Client;

pub const USER_AGENT: &str = concat!("rattler/", env!("CARGO_PKG_VERSION"));

/// Returns the cache action to use for repodata queries in the CLI.
pub fn repodata_cache_action(offline: bool) -> CacheAction {
    if offline {
        CacheAction::ForceCacheOnly
    } else {
        CacheAction::default()
    }
}

/// Creates the HTTP client with the middleware stack used by the CLI.
///
/// Includes [`AuthChallengeMiddleware`] with its default flows: a
/// `WWW-Authenticate` challenge from a prefix.dev host mints a token via
/// CI OIDC and replays the request (prefix-dev/pixi#6318). Stored
/// credentials from [`AuthenticationMiddleware`] take precedence.
pub fn create_client_with_middleware(
    offline: bool,
) -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    let download_client = Client::builder()
        .no_gzip()
        .user_agent(USER_AGENT)
        .build()
        .into_diagnostic()
        .context("failed to create HTTP client")?;

    let authentication_storage =
        AuthenticationStorage::from_env_and_defaults().into_diagnostic()?;

    let client = reqwest_middleware::ClientBuilder::new(download_client.clone());
    let client = if offline {
        client.with(OfflineMiddleware)
    } else {
        client
    };
    let client = client
        .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
            authentication_storage.clone(),
        )))
        .with_arc(Arc::new(AuthChallengeMiddleware::default()));

    let client = client.with(rattler_networking::OciMiddleware::new(download_client));
    #[cfg(feature = "s3")]
    let client = client.with(rattler_networking::S3Middleware::new(
        HashMap::new(),
        authentication_storage,
    ));
    #[cfg(feature = "gcs")]
    let client = client.with(rattler_networking::GCSMiddleware::default());

    Ok(client.build())
}
