use crate::ExcludeNewer;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

/// The conda channel that was used for the dependency
#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Channel {
    /// The URL of the channel. File paths are also supported.
    pub url: String,
    /// A channel-specific exclude-newer override.
    #[serde(rename = "exclude-newer")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_newer: Option<ExcludeNewer>,
    /// Used env vars for the channel (e.g. hints for passwords or other secrets)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "crate::utils::serde::Ordered<_>")]
    pub used_env_vars: Vec<String>,
}

impl From<String> for Channel {
    fn from(url: String) -> Self {
        Self {
            url,
            exclude_newer: None,
            used_env_vars: Vec::default(),
        }
    }
}

impl From<&str> for Channel {
    fn from(url: &str) -> Self {
        Self {
            url: url.to_string(),
            exclude_newer: None,
            used_env_vars: Vec::default(),
        }
    }
}
