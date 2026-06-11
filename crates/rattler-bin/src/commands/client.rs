use std::{collections::HashMap, sync::Arc};

use miette::{Context, IntoDiagnostic};
use rattler_conda_types::Channel;
use rattler_networking::{
    AuthChallengeMiddleware, AuthenticationMiddleware, AuthenticationStorage,
    trusted_publishing::{TrustedPublishingFlow, TrustedPublishingOptions},
};
use reqwest::Client;
use url::Url;

pub const USER_AGENT: &str = concat!("rattler/", env!("CARGO_PKG_VERSION"));

/// Hosts for which the CLI is willing to perform CI OIDC trusted publishing.
/// Restricting this keeps the CLI from volunteering CI identity tokens to
/// arbitrary channel hosts.
fn is_prefix_dev_host(host: &str) -> bool {
    host == "prefix.dev" || host.ends_with(".prefix.dev")
}

/// Creates an HTTP client with the middleware stack used by the CLI for remote fetches.
///
/// This stack performs no challenge/trusted-publishing auth; commands that
/// read channels should prefer [`create_client_with_middleware_for_channels`].
pub fn create_client_with_middleware() -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    create_client_with_middleware_for_channels(&[])
}

/// Like [`create_client_with_middleware`], but additionally layers one
/// [`AuthChallengeMiddleware`] per unique `https` channel host that matches
/// the prefix.dev host policy. On a `WWW-Authenticate` challenge from such a
/// host, the middleware acquires a token via CI OIDC trusted publishing
/// (audience = the channel host) and replays the request.
///
/// This is the reference wiring for challenge-reactive private-channel
/// reads (see prefix-dev/pixi#6318).
pub fn create_client_with_middleware_for_channels(
    channels: &[Channel],
) -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    let download_client = Client::builder()
        .no_gzip()
        .user_agent(USER_AGENT)
        .build()
        .into_diagnostic()
        .context("failed to create HTTP client")?;

    let authentication_storage =
        AuthenticationStorage::from_env_and_defaults().into_diagnostic()?;

    let mut client =
        reqwest_middleware::ClientBuilder::new(download_client.clone()).with_arc(Arc::new(
            AuthenticationMiddleware::from_auth_storage(authentication_storage.clone()),
        ));

    // The mint exchange must not itself go through AuthChallengeMiddleware
    // (it would recurse), so it uses a plain client.
    let mint_client = reqwest_middleware::ClientBuilder::new(download_client.clone()).build();

    let mut seen_hosts = std::collections::HashSet::new();
    for channel in channels {
        let url: &Url = channel.base_url.as_ref();
        if url.scheme() != "https" {
            continue;
        }
        let Some(host) = url.host_str() else {
            continue;
        };
        if !is_prefix_dev_host(host)
            || !seen_hosts.insert((host.to_string(), url.port_or_known_default()))
        {
            continue;
        }
        let Some(options) = TrustedPublishingOptions::for_host(url) else {
            continue;
        };
        let mut server = url.clone();
        // Cosmetic: the middleware scopes on scheme+host+port and ignores
        // path/query; normalizing just keeps Debug output tidy.
        server.set_path("/");
        server.set_query(None);
        client = client.with_arc(Arc::new(AuthChallengeMiddleware::new(
            server,
            Arc::new(TrustedPublishingFlow::new(options, mint_client.clone())),
        )));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_dev_host_policy() {
        assert!(is_prefix_dev_host("prefix.dev"));
        assert!(is_prefix_dev_host("beta.prefix.dev"));
        assert!(is_prefix_dev_host("staging.beta.prefix.dev"));
        // not subdomains of prefix.dev:
        assert!(!is_prefix_dev_host("evil-prefix.dev"));
        assert!(!is_prefix_dev_host("prefix.dev.evil.com"));
        assert!(!is_prefix_dev_host("conda.anaconda.org"));
        // trailing-dot (DNS absolute) hosts are deliberately not matched:
        // url::Url preserves the dot, so this fails closed.
        assert!(!is_prefix_dev_host("beta.prefix.dev."));
    }
}
