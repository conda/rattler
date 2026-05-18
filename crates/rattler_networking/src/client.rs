//! Helpers for constructing reqwest clients and middleware chains from a
//! [`rattler_config::config::ConfigBase`].
//!
//! The two entry points here pair up:
//! - [`apply_config_to_reqwest_builder`] applies the TLS and proxy settings
//!   that belong on the raw [`reqwest::ClientBuilder`].
//! - [`client_with_middleware_from_config`] wraps an already-built
//!   [`reqwest::Client`] with the standard authentication, mirror, OCI, S3,
//!   and GCS middlewares using config-derived data. The S3 and GCS layers
//!   are only added when the corresponding cargo features are enabled.
//!
//! Callers that need finer control (custom user-agent, extra middlewares,
//! shared semaphores, ...) can still build everything by hand — these helpers
//! exist for the common case where the rattler config file is the source of
//! truth.

use reqwest_middleware::ClientWithMiddleware;

use crate::{AuthenticationMiddleware, AuthenticationStorage, MirrorMiddleware, OciMiddleware};

#[cfg(feature = "s3")]
use crate::S3Middleware;

#[cfg(feature = "gcs")]
use crate::GCSMiddleware;

/// Apply TLS and proxy settings from a config to a [`reqwest::ClientBuilder`].
///
/// - `tls-no-verify = true` is translated to
///   [`reqwest::ClientBuilder::danger_accept_invalid_certs`].
/// - `proxy-config.http` / `proxy-config.https` add the corresponding
///   [`reqwest::Proxy`] entries; `proxy-config.non-proxy-hosts` is parsed as a
///   comma-joined `NoProxy` list and attached to both proxies.
///
/// Other reqwest knobs (user-agent, timeouts, gzip handling, ...) are left to
/// the caller.
pub fn apply_config_to_reqwest_builder<T>(
    mut builder: reqwest::ClientBuilder,
    config: &rattler_config::config::ConfigBase<T>,
) -> reqwest::Result<reqwest::ClientBuilder>
where
    T: rattler_config::config::Config + Default,
{
    if config.tls_no_verify.unwrap_or(false) {
        builder = builder.danger_accept_invalid_certs(true);
    }

    let no_proxy = if config.proxy_config.non_proxy_hosts.is_empty() {
        None
    } else {
        reqwest::NoProxy::from_string(&config.proxy_config.non_proxy_hosts.join(","))
    };

    if let Some(http_url) = config.proxy_config.http.as_ref() {
        let proxy = reqwest::Proxy::http(http_url.as_str())?.no_proxy(no_proxy.clone());
        builder = builder.proxy(proxy);
    }
    if let Some(https_url) = config.proxy_config.https.as_ref() {
        let proxy = reqwest::Proxy::https(https_url.as_str())?.no_proxy(no_proxy);
        builder = builder.proxy(proxy);
    }

    Ok(builder)
}

/// Build a [`ClientWithMiddleware`] from a config, wrapping the provided
/// reqwest client with the standard middleware chain.
///
/// The chain is:
/// 1. [`AuthenticationMiddleware`] backed by `auth_storage`.
/// 2. [`MirrorMiddleware`] built from `config.mirrors` (skipped if empty).
/// 3. [`OciMiddleware`] using `client` as the inner transport.
/// 4. [`S3Middleware`] built from `config.s3_options` (only with the `s3`
///    feature).
/// 5. [`GCSMiddleware::default`] (only with the `gcs` feature).
///
/// Pair this with [`apply_config_to_reqwest_builder`] to keep TLS, proxy, and
/// middleware setup driven entirely by the config:
///
/// ```ignore
/// let auth = AuthenticationStorage::from_config(&config)?;
/// let reqwest_client = apply_config_to_reqwest_builder(
///     reqwest::Client::builder().user_agent("my-app/1.0"),
///     &config,
/// )?
/// .build()?;
/// let client = client_with_middleware_from_config(reqwest_client, &config, auth);
/// ```
pub fn client_with_middleware_from_config<T>(
    client: reqwest::Client,
    config: &rattler_config::config::ConfigBase<T>,
    auth_storage: AuthenticationStorage,
) -> ClientWithMiddleware
where
    T: rattler_config::config::Config + Default,
{
    let mut builder = reqwest_middleware::ClientBuilder::new(client.clone()).with(
        AuthenticationMiddleware::from_auth_storage(auth_storage.clone()),
    );

    if !config.mirrors.is_empty() {
        builder = builder.with(MirrorMiddleware::from_config(config));
    }

    builder = builder.with(OciMiddleware::new(client));

    #[cfg(feature = "s3")]
    {
        builder = builder.with(S3Middleware::from_config(config, auth_storage));
    }
    // Silence unused-variable warnings when the s3 feature is off.
    #[cfg(not(feature = "s3"))]
    let _ = auth_storage;

    #[cfg(feature = "gcs")]
    {
        builder = builder.with(GCSMiddleware::default());
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    use rattler_config::config::ConfigBase;

    #[test]
    fn applies_tls_no_verify_and_proxy() {
        let mut config = ConfigBase::<()>::default();
        config.tls_no_verify = Some(true);
        config.proxy_config.https = Some("https://proxy.example.com:8080".parse().unwrap());
        config.proxy_config.http = Some("http://proxy.example.com:8080".parse().unwrap());
        config.proxy_config.non_proxy_hosts = vec!["localhost".into(), "127.0.0.1".into()];

        let builder = apply_config_to_reqwest_builder(reqwest::Client::builder(), &config).unwrap();
        // We can't introspect ClientBuilder, but it should build successfully.
        let _client = builder.build().expect("client builds with proxies + tls");
    }

    #[test]
    fn empty_config_is_a_no_op() {
        let config = ConfigBase::<()>::default();
        let builder = apply_config_to_reqwest_builder(reqwest::Client::builder(), &config).unwrap();
        let _client = builder.build().unwrap();
    }

    #[test]
    fn builds_middleware_chain_from_config() {
        let mut config = ConfigBase::<()>::default();
        config.mirrors.insert(
            "https://conda.anaconda.org".parse().unwrap(),
            vec!["https://mirror.example.com".parse().unwrap()],
        );

        let client = reqwest::Client::builder().build().unwrap();
        let _wrapped =
            client_with_middleware_from_config(client, &config, AuthenticationStorage::empty());
    }
}
