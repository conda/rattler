use std::fmt::{Debug, Display, Formatter};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{utils::url_with_trailing_slash::UrlWithTrailingSlash, Platform};

/// Represents a channel base url. This is a wrapper around an url that is
/// normalized:
///
/// * The URL always contains a trailing `/`.
///
/// This is useful to be able to compare different channels.
#[derive(Clone, Hash, Eq, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct ChannelUrl(UrlWithTrailingSlash);

impl ChannelUrl {
    /// Returns the base Url of the channel.
    pub fn url(&self) -> &UrlWithTrailingSlash {
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

// Override the behavior of the `Serialize` trait to remove the trailing slash.
// In code, we want to ensure that a trailing slash is always present but when
// we serialize the type it can be removed to safe space and make it look better
// for humans.
impl Serialize for ChannelUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_str().trim_end_matches('/').serialize(serializer)
    }
}

impl Debug for ChannelUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}

impl From<Url> for ChannelUrl {
    fn from(url: Url) -> Self {
        Self(UrlWithTrailingSlash::from(url))
    }
}

impl From<ChannelUrl> for Url {
    fn from(value: ChannelUrl) -> Self {
        value.0.into()
    }
}

impl AsRef<Url> for ChannelUrl {
    fn as_ref(&self) -> &Url {
        &self.0
    }
}

impl Display for ChannelUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
