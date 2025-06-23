use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    config::{Config, MergeError, ValidationError},
    edit::ConfigEditError,
};

// detect proxy env vars like curl: https://curl.se/docs/manpage.html
static ENV_HTTP_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    ["http_proxy", "all_proxy", "ALL_PROXY"]
        .iter()
        .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
});
static ENV_HTTPS_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
        .iter()
        .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
});
static ENV_NO_PROXY: LazyLock<Option<String>> = LazyLock::new(|| {
    ["no_proxy", "NO_PROXY"]
        .iter()
        .find_map(|&k| std::env::var(k).ok().filter(|v| !v.is_empty()))
});
static USE_PROXY_FROM_ENV: LazyLock<bool> =
    LazyLock::new(|| (*ENV_HTTPS_PROXY).is_some() || (*ENV_HTTP_PROXY).is_some());

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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

impl Default for ProxyConfig {
    fn default() -> Self {
        if *USE_PROXY_FROM_ENV {
            Self {
                https: ENV_HTTPS_PROXY.as_ref().and_then(|s| Url::parse(s).ok()),
                http: ENV_HTTP_PROXY.as_ref().and_then(|s| Url::parse(s).ok()),
                non_proxy_hosts: ENV_NO_PROXY
                    .as_ref()
                    .map(|s| s.split(',').map(String::from).collect())
                    .unwrap_or_default(),
            }
        } else {
            Self {
                https: None,
                http: None,
                non_proxy_hosts: Vec::new(),
            }
        }
    }
}

impl ProxyConfig {
    pub fn is_default(&self) -> bool {
        self.https.is_none() && self.https.is_none() && self.non_proxy_hosts.is_empty()
    }
}

impl Config for ProxyConfig {
    fn get_extension_name(&self) -> String {
        "proxy".to_string()
    }

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
        if self.https.is_none() && self.http.is_none() {
            return Err(ValidationError::Invalid(
                "At least one of https or http proxy must be set".to_string(),
            ));
        }
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec![
            "https".to_string(),
            "http".to_string(),
            "non-proxy-hosts".to_string(),
        ]
    }

    fn set(
        &mut self,
        key: &str,
        value: Option<String>,
    ) -> Result<(), crate::config::ConfigEditError> {
        if key == "proxy-config" {
            if let Some(value) = value {
                *self = serde_json::de::from_str(&value).map_err(|e| {
                    ConfigEditError::JsonParseError {
                        key: key.to_string(),
                        source: e,
                    }
                })?;
            } else {
                *self = ProxyConfig::default();
            }
            return Ok(());
        } else if !key.starts_with("proxy-config.") {
            return Err(ConfigEditError::UnknownKeyInner {
                key: key.to_string(),
            });
        }

        let subkey = key.strip_prefix("proxy-config.").unwrap();
        match subkey {
            "https" => {
                self.https = value
                    .map(|v| {
                        Url::parse(&v).map_err(|e| ConfigEditError::UrlParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?;
            }
            "http" => {
                self.http = value
                    .map(|v| {
                        Url::parse(&v).map_err(|e| ConfigEditError::UrlParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?;
            }
            "non-proxy-hosts" => {
                self.non_proxy_hosts = value
                    .map(|v| {
                        serde_json::de::from_str(&v).map_err(|e| ConfigEditError::JsonParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?
                    .unwrap_or_default();
            }
            _ => {
                return Err(ConfigEditError::UnknownKeyInner {
                    key: key.to_string(),
                })
            }
        }
        Ok(())
    }
}
