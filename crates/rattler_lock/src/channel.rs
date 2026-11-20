use serde::{Deserialize, Serialize};
use serde_with::serde_as;

/// The conda channel that was used for the dependency
#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Channel {
    /// The URL of the channel. File paths are also supported.
    pub url: String,
    /// Used env vars for the channel (e.g. hints for passwords or other secrets)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "crate::utils::serde::Ordered<_>")]
    pub used_env_vars: Vec<String>,
    /// The name of the multichannel the channel was requested through, or
    /// `None` if it was requested on its own.
    ///
    /// The channels of a multichannel are listed next to each other, in
    /// order of preference, and share a single channel priority tier when
    /// solving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multichannel: Option<String>,
}

impl Channel {
    /// Marks this channel as a member of the multichannel called `name`.
    pub fn with_multichannel(mut self, name: impl Into<String>) -> Self {
        self.multichannel = Some(name.into());
        self
    }
}

impl From<String> for Channel {
    fn from(url: String) -> Self {
        Self {
            url,
            used_env_vars: Vec::default(),
            multichannel: None,
        }
    }
}

impl From<&str> for Channel {
    fn from(url: &str) -> Self {
        Self {
            url: url.to_string(),
            used_env_vars: Vec::default(),
            multichannel: None,
        }
    }
}
