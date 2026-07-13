use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::{Config, MergeError, ValidationError};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ProxyConfig {
    /// The HTTPS proxy to use
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub https: Option<Url>,

    /// The HTTP proxy to use
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<Url>,

    /// A list of no proxy pattern
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub non_proxy_hosts: Vec<String>,
}

impl ProxyConfig {
    /// Read the proxy configuration from the standard environment
    /// variables, like curl does: <https://curl.se/docs/manpage.html>.
    ///
    /// Returns an empty configuration when no proxy variables are set.
    /// Note that `Default::default()` deliberately does *not* consult the
    /// environment: a default-constructed configuration must be empty so
    /// that it can be serialized, merged and compared without leaking
    /// machine state. Consumers that want the environment behavior merge
    /// this on top explicitly.
    pub fn from_env() -> Self {
        let env = |keys: &[&str]| {
            keys.iter()
                .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        };
        let http = env(&["http_proxy", "all_proxy", "ALL_PROXY"]);
        let https = env(&["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]);
        if http.is_none() && https.is_none() {
            return Self::default();
        }
        Self {
            https: https.and_then(|s| Url::parse(&s).ok()),
            http: http.and_then(|s| Url::parse(&s).ok()),
            non_proxy_hosts: env(&["no_proxy", "NO_PROXY"])
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_default(),
        }
    }

    pub fn is_default(&self) -> bool {
        self.https.is_none() && self.http.is_none() && self.non_proxy_hosts.is_empty()
    }
}

impl Config for ProxyConfig {
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            https: other.https.as_ref().or(self.https.as_ref()).cloned(),
            http: other.http.as_ref().or(self.http.as_ref()).cloned(),
            non_proxy_hosts: if other.is_default() {
                self.non_proxy_hosts.clone()
            } else {
                other.non_proxy_hosts.clone()
            },
        })
    }

    fn validate(&self) -> Result<(), ValidationError> {
        // Empty is valid — no proxy configured is the common case.
        // Downstreams (e.g. pixi) emit their own informational warning
        // when `non_proxy_hosts` is set without an actual proxy URL.
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec![
            "https".to_string(),
            "http".to_string(),
            "non-proxy-hosts".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unconfigured `ProxyConfig` must validate. Previously
    /// `validate` rejected the default-empty state, which broke any
    /// caller that ran validation on a config without proxies.
    #[test]
    fn validate_accepts_empty() {
        let config = ProxyConfig {
            https: None,
            http: None,
            non_proxy_hosts: vec![],
        };
        config.validate().expect("empty proxy config is valid");
    }
}
