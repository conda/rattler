use std::{sync::Arc, time::Duration};

use miette::{Context, IntoDiagnostic};
use rattler_networking::{
    AuthChallengeMiddleware, AuthenticationMiddleware, AuthenticationStorage, OfflineMiddleware,
};
use rattler_repodata_gateway::fetch::CacheAction;
use reqwest::Client;
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};

pub const USER_AGENT: &str = concat!("rattler/", env!("CARGO_PKG_VERSION"));

/// How long establishing a connection may take before the attempt is aborted.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a read from a response may stall before the request is aborted.
/// This bounds a hung server without limiting the total duration of large
/// downloads that are still making progress.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// How often a request that failed with a transient error is retried.
const RETRIES: u32 = 3;

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
///
/// Transient failures are retried with exponential backoff, and connect and
/// read timeouts bound how long a hung server can stall a request. When the
/// `s3` feature is enabled, S3 buckets configured through the `s3-options`
/// of the rattler configuration are picked up as well.
pub fn create_client_with_middleware(
    offline: bool,
) -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    let download_client = Client::builder()
        .no_gzip()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
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
    // The retry middleware is added before the other middlewares so that a
    // retried request passes through authentication (and the URL rewriting
    // middlewares) again.
    let client = client.with(RetryTransientMiddleware::new_with_policy(
        ExponentialBackoff::builder().build_with_max_retries(RETRIES),
    ));
    let client = client
        .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
            authentication_storage.clone(),
        )))
        .with_arc(Arc::new(AuthChallengeMiddleware::default()));

    let client = client.with(
        rattler_networking::OciMiddleware::new(download_client)
            .with_authentication_storage(authentication_storage.clone()),
    );
    #[cfg(feature = "s3")]
    let client = {
        let config = super::gateway::load_config()?;
        client.with(rattler_networking::S3Middleware::new(
            rattler_networking::s3_middleware::compute_s3_config_from_config(&config.common),
            authentication_storage,
        ))
    };
    #[cfg(feature = "gcs")]
    let client = client.with(rattler_networking::GCSMiddleware::default());

    Ok(client.build())
}
