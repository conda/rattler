//! Apply the networking settings of the shared rattler configuration to a
//! [`reqwest::ClientBuilder`].

use rattler_config::config::{CommonConfig, proxy::ProxyConfig, tls::TlsRootCerts};

use crate::proxy::proxies_from_config;
#[cfg(any(feature = "native-tls", feature = "rustls"))]
use crate::tls::Certificates;

/// The [`TlsRootCerts`] mode used when the configuration does not set one.
///
/// `native-tls` always talks to the OS trust store, so the default is
/// [`TlsRootCerts::System`]. With `rustls` the bundled Mozilla roots are
/// portable and work without any platform integration, so the default is
/// [`TlsRootCerts::Webpki`].
pub const fn default_tls_root_certs() -> TlsRootCerts {
    if cfg!(all(feature = "native-tls", not(feature = "rustls"))) {
        TlsRootCerts::System
    } else {
        TlsRootCerts::Webpki
    }
}

/// Create a [`reqwest::ClientBuilder`] with the networking settings of a
/// [`CommonConfig`] applied.
///
/// This configures:
/// - The TLS root certificates according to `tls-root-certs` (falling back to
///   [`default_tls_root_certs`]), honoring `SSL_CERT_FILE` / `SSL_CERT_DIR`.
/// - Certificate verification according to `tls-no-verify`.
/// - The proxies from `proxy-config`. When proxy environment variables are
///   set they take precedence and reqwest picks them up itself.
///
/// Settings such as the user agent and timeouts are left to the caller.
pub fn client_builder_from_config(
    config: &CommonConfig,
) -> Result<reqwest::ClientBuilder, reqwest::Error> {
    let mut builder = reqwest::Client::builder();

    #[cfg(any(feature = "native-tls", feature = "rustls"))]
    {
        let tls_root_certs = config.tls_root_certs.unwrap_or_else(default_tls_root_certs);

        // rustls has no OS trust store, so it needs explicit anchors via
        // `tls_certs_only`. native-tls already uses the OS store; routing
        // System mode through `tls_certs_only` would disable the built-in
        // roots and reject enterprise/proxy CAs the OS trusts, so we keep the
        // OS store and only merge the roots from the environment there.
        #[cfg(feature = "rustls")]
        {
            builder = builder
                .tls_backend_rustls()
                .tls_certs_only(Certificates::for_mode(tls_root_certs).to_reqwest_certs());
        }
        #[cfg(all(feature = "native-tls", not(feature = "rustls")))]
        {
            builder = apply_native_tls_roots(builder.tls_backend_native(), tls_root_certs);
        }

        let tls_no_verify = config.tls_no_verify.unwrap_or(false);
        if tls_no_verify {
            tracing::warn!(
                "TLS verification is disabled. This is insecure and should only be used for testing or internal networks."
            );
        }
        builder = builder.tls_danger_accept_invalid_certs(tls_no_verify);
    }

    if ProxyConfig::from_env().is_default() {
        for proxy in proxies_from_config(&config.proxy_config)? {
            builder = builder.proxy(proxy);
        }
    } else if !config.proxy_config.is_default() {
        tracing::info!("proxy configs are overridden by proxy environment vars.");
    }

    Ok(builder)
}

/// Configure the root certificates for the native-tls backend.
///
/// System keeps the OS store and merges any `SSL_CERT_FILE` / `SSL_CERT_DIR`
/// roots. Webpki replaces the OS store with the bundled Mozilla roots.
#[cfg(all(feature = "native-tls", not(feature = "rustls")))]
fn apply_native_tls_roots(
    builder: reqwest::ClientBuilder,
    mode: TlsRootCerts,
) -> reqwest::ClientBuilder {
    let env_certs = Certificates::from_env();
    match mode {
        TlsRootCerts::Webpki => {
            let mut certs = Certificates::webpki_roots();
            if let Some(env_certs) = env_certs {
                certs.merge(env_certs);
            }
            builder.tls_certs_only(certs.to_reqwest_certs())
        }
        TlsRootCerts::System => match env_certs {
            Some(env_certs) => builder.tls_certs_merge(env_certs.to_reqwest_certs()),
            None => builder,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_builds() {
        client_builder_from_config(&CommonConfig::default())
            .unwrap()
            .build()
            .unwrap();
    }

    #[test]
    fn configured_settings_build() {
        let config = CommonConfig {
            tls_no_verify: Some(true),
            tls_root_certs: Some(TlsRootCerts::System),
            proxy_config: ProxyConfig {
                https: Some("http://proxy.example.com:8080".parse().unwrap()),
                http: None,
                non_proxy_hosts: vec!["localhost".to_string()],
            },
            ..CommonConfig::default()
        };
        client_builder_from_config(&config)
            .unwrap()
            .build()
            .unwrap();
    }
}
