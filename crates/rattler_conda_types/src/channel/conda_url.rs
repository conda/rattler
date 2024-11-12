use std::fmt::{Display, Formatter};

use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

use crate::Platform;

/// Represents a channel base url. This is a wrapper around an url that is
/// normalized:
///
/// * The URL always contains a trailing `/`.
///
/// This is useful to be able to compare different channels.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CondaUrl(Url);

impl CondaUrl {
    /// Returns the base Url of the channel.
    pub fn url(&self) -> &Url {
        &self.0
    }

    /// Returns the string representation of the url.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Append the platform to the base url.
    pub fn platform_url(&self, platform: Platform) -> Url {
        self.0
            .join(&format!("{}/", platform.as_str())) // trailing slash is important here as this signifies a directory
            .expect("platform is a valid url fragment")
    }
}

impl<'de> Deserialize<'de> for CondaUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let url = Url::deserialize(deserializer)?;
        Ok(url.into())
    }
}

impl From<Url> for CondaUrl {
    fn from(url: Url) -> Self {
        let path = url.path();
        if path.ends_with('/') {
            Self(url)
        } else {
            let mut url = url.clone();
            url.set_path(&format!("{path}/"));
            Self(url)
        }
    }
}

impl From<CondaUrl> for Url {
    fn from(value: CondaUrl) -> Self {
        value.0
    }
}

impl AsRef<Url> for CondaUrl {
    fn as_ref(&self) -> &Url {
        &self.0
    }
}

impl Display for CondaUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
