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
    pub fn to_string(&self) -> String {
        let mut url = self.0.as_ref().clone();
        url.set_username("").ok();
        url.set_password(None).ok();

        // Remove a `/t/token` from the url if it exists.
        let path = url.path();
        if path.starts_with("/t/") {
            let mut parts = path.splitn(4, '/');
            let _ = parts.next();
            let _ = parts.next();
            let _ = parts.next();
            url.set_path(parts.collect::<Vec<_>>().join("/").as_str());
        }

        url.to_string()
    }

    /// Returns the "raw" string representation of the url which might contain
    /// credentials or tokens.
    pub fn as_str_with_secrets(&self) -> &str {
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
        self.to_string().trim_end_matches('/').serialize(serializer)
    }
}

impl Debug for ChannelUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::url_with_trailing_slash::UrlWithTrailingSlash;

    #[test]
    fn test_channel_url() {
        let url = Url::parse("https://repo.anaconda.com/pkgs/main/").unwrap();
        let channel_url = ChannelUrl::from(url.clone());
        assert_eq!(channel_url.url(), &UrlWithTrailingSlash::from(url));
        assert_eq!(channel_url.to_string(), "https://repo.anaconda.com/pkgs/main/");
    }

    #[test]
    fn test_url_with_credentials() {
        let url = Url::parse("https://user:pass@repo.anaconda.com/pkgs/main/").unwrap();
        let channel_url = ChannelUrl::from(url);
        assert_eq!(channel_url.to_string(), "https://repo.anaconda.com/pkgs/main/");
        assert_eq!(channel_url.as_str_with_secrets(), "https://user:pass@repo.anaconda.com/pkgs/main/");
    }

    #[test]
    fn test_url_with_token() {
        let url = Url::parse("https://repo.anaconda.com/t/secret-token/pkgs/main/").unwrap();
        let channel_url = ChannelUrl::from(url);
        assert_eq!(channel_url.to_string(), "https://repo.anaconda.com/pkgs/main/");
        assert_eq!(channel_url.as_str_with_secrets(), "https://repo.anaconda.com/t/secret-token/pkgs/main/");
    }

    #[test]
    fn test_platform_url() {
        let url = Url::parse("https://repo.anaconda.com/pkgs/main/").unwrap();
        let channel_url = ChannelUrl::from(url);
        assert_eq!(
            channel_url.platform_url(Platform::Linux64).as_str(),
            "https://repo.anaconda.com/pkgs/main/linux-64/"
        );
    }
}