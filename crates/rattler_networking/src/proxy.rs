//! Convert the proxy settings of the shared rattler configuration into
//! [`reqwest::Proxy`] instances.

use rattler_config::config::proxy::ProxyConfig;
use reqwest::NoProxy;

/// Build the [`reqwest::Proxy`] instances described by a [`ProxyConfig`],
/// honoring its `non-proxy-hosts` exclusion list.
///
/// Returns an empty vector when no proxy is configured. Note that
/// [`ProxyConfig::default`] is empty; combine with
/// [`ProxyConfig::from_env`] first if the environment variables should be
/// taken into account (reqwest also applies proxy environment variables
/// itself unless [`reqwest::ClientBuilder::no_proxy`] is used).
pub fn proxies_from_config(config: &ProxyConfig) -> Result<Vec<reqwest::Proxy>, reqwest::Error> {
    let mut proxies = Vec::new();
    if config.is_default() {
        return Ok(proxies);
    }

    let no_proxy = (!config.non_proxy_hosts.is_empty())
        .then(|| NoProxy::from_string(&config.non_proxy_hosts.join(",")));

    match (&config.http, &config.https) {
        (Some(http), Some(https)) if http == https => {
            proxies.push(reqwest::Proxy::all(http.clone())?.no_proxy(no_proxy.flatten()));
        }
        (http, https) => {
            if let Some(http) = http {
                proxies
                    .push(reqwest::Proxy::http(http.clone())?.no_proxy(no_proxy.clone().flatten()));
            }
            if let Some(https) = https {
                proxies.push(reqwest::Proxy::https(https.clone())?.no_proxy(no_proxy.flatten()));
            }
        }
    }

    Ok(proxies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_yields_no_proxies() {
        let proxies = proxies_from_config(&ProxyConfig::default()).unwrap();
        assert!(proxies.is_empty());
    }

    #[test]
    fn same_url_for_http_and_https_yields_single_proxy() {
        let url = url::Url::parse("http://proxy.example.com:8080").unwrap();
        let config = ProxyConfig {
            http: Some(url.clone()),
            https: Some(url),
            non_proxy_hosts: vec!["localhost".to_string()],
        };
        let proxies = proxies_from_config(&config).unwrap();
        assert_eq!(proxies.len(), 1);
    }

    #[test]
    fn distinct_urls_yield_two_proxies() {
        let config = ProxyConfig {
            http: Some(url::Url::parse("http://proxy.example.com:8080").unwrap()),
            https: Some(url::Url::parse("http://secure-proxy.example.com:8080").unwrap()),
            non_proxy_hosts: vec![],
        };
        let proxies = proxies_from_config(&config).unwrap();
        assert_eq!(proxies.len(), 2);
    }
}
