use super::{ParsePlatformError, Platform};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    channel_alias: Url,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        ChannelConfig {
            channel_alias: Url::from_str("https://conda.anaconda.org")
                .expect("could not parse default channel alias"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub platforms: Vec<Platform>,
    pub scheme: String,
    pub location: String,
    pub name: String,
}

impl Channel {
    pub fn from_str(str: &str, config: &ChannelConfig) -> Result<Self, ParseChannelError> {
        let (platforms, channel) = parse_platforms(str)?;

        let channel = if parse_scheme(channel).is_some() {
            let url = Url::parse(channel)?;
            Channel::from_url(url, platforms, config)
        } else if is_path(channel) {
            let path = PathBuf::from(channel);
            let url =
                Url::from_file_path(&path).map_err(|_| ParseChannelError::InvalidPath(path))?;
            Channel::from_url(url, platforms, config)
        } else {
            Channel::from_name(channel, platforms, config)
        };

        Ok(channel)
    }

    /// Constructs a new `Channel` from a `Url` and associated platforms.
    pub fn from_url(url: Url, platforms: Vec<Platform>, _config: &ChannelConfig) -> Self {
        let path = url.path().trim_end_matches('/');

        // Case 1: No path give, channel name is ""
        if path.is_empty() {
            return Self {
                platforms,
                scheme: url.scheme().to_owned(),
                location: url.host_str().unwrap_or("").to_owned(),
                name: String::from(""),
            };
        }

        // Case 2: migrated_custom_channels
        // Case 3: migrated_channel_aliases
        // Case 4: custom_channels matches
        // Case 5: channel_alias match

        if let Some(host) = url.host_str() {
            // Case 7: Fallback
            let location = if let Some(port) = url.port() {
                format!("{}:{}", host, port)
            } else {
                host.to_owned()
            };
            Self {
                platforms,
                scheme: url.scheme().to_owned(),
                location,
                name: path.trim_start_matches('/').to_owned(),
            }
        } else {
            // Case 6: non-otherwise-specified file://-type urls
            let (location, name) = url
                .path()
                .rsplit_once('/')
                .unwrap_or_else(|| ("/", url.path()));
            Self {
                platforms,
                scheme: String::from("file"),
                location: location.to_owned(),
                name: name.to_owned(),
            }
        }
    }

    /// Construct a channel from a name
    pub fn from_name(name: &str, platforms: Vec<Platform>, config: &ChannelConfig) -> Self {
        // TODO: custom channels
        Self {
            platforms,
            scheme: config.channel_alias.scheme().to_owned(),
            location: format!(
                "{}/{}",
                config.channel_alias.host_str().unwrap_or("/").to_owned(),
                config.channel_alias.path()
            )
            .trim_end_matches('/')
            .to_owned(),
            name: name.to_owned(),
        }
    }

    /// Returns the base Url of the channel. This does not include the platform part.
    pub fn base_url(&self) -> Url {
        Url::from_str(&format!(
            "{}://{}/{}",
            self.scheme, self.location, self.name
        ))
        .expect("could not construct base_url for channel")
    }

    /// Returns the Urls for the given platform
    fn platform_url(&self, platform: Platform) -> Url {
        let mut base_url = self.base_url();
        base_url.set_path(&format!("{}/{}/", base_url.path(), platform.as_str()));
        base_url
    }

    /// Returns the Urls for all the supported platforms of this package.
    pub fn platforms_url(&self) -> Vec<(Platform, Url)> {
        self.platforms
            .iter()
            .map(|&platform| (platform, self.platform_url(platform)))
            .collect()
    }
}

#[derive(Debug, Error)]
pub enum ParseChannelError {
    #[error("could not parse the platforms")]
    ParsePlatformError(#[source] ParsePlatformError),

    #[error("could not parse url")]
    ParseUrlError(#[source] url::ParseError),

    #[error("invalid path '{0}")]
    InvalidPath(PathBuf),
}

impl From<ParsePlatformError> for ParseChannelError {
    fn from(err: ParsePlatformError) -> Self {
        ParseChannelError::ParsePlatformError(err)
    }
}

impl From<url::ParseError> for ParseChannelError {
    fn from(err: url::ParseError) -> Self {
        ParseChannelError::ParseUrlError(err)
    }
}

/// Extract the platforms from the given human readable channel.
fn parse_platforms(channel: &str) -> Result<(Vec<Platform>, &str), ParsePlatformError> {
    if channel.rfind(']').is_some() {
        if let Some(start_platform_idx) = channel.find('[') {
            let platform_part = &channel[start_platform_idx + 1..channel.len() - 1];
            let platforms = platform_part
                .split(',')
                .map(str::trim)
                .map(FromStr::from_str)
                .collect::<Result<_, _>>()?;
            return Ok((platforms, &channel[0..start_platform_idx]));
        }
    }

    Ok((default_platforms(), channel))
}

/// Returns the default platforms. These are based on the platform this binary was build for as well
/// as platform agnostic platforms.
fn default_platforms() -> Vec<Platform> {
    return vec![Platform::current(), Platform::NoArch];
}

/// Parses the schema part of the human-readable channel. Returns the scheme part if it exists.
fn parse_scheme(channel: &str) -> Option<&str> {
    let scheme_end = channel.find("://")?;

    // Scheme part is too long
    if scheme_end > 11 {
        return None;
    }

    let scheme_part = &channel[0..scheme_end];
    let mut scheme_chars = scheme_part.chars();

    // First character must be alphabetic
    if scheme_chars.next().map(char::is_alphabetic) != Some(true) {
        return None;
    }

    // The rest must be alpha-numeric
    if scheme_chars.all(char::is_alphanumeric) {
        Some(scheme_part)
    } else {
        None
    }
}

/// Returns true if the specified string is considered to be a path
fn is_path(path: &str) -> bool {
    let re = regex::Regex::new(r"(\./|\.\.|~|/|[a-zA-Z]:[/\\]|\\\\|//)").unwrap();
    re.is_match(path)
}

#[cfg(test)]
mod tests {
    use crate::conda::channel::parse_scheme;

    #[test]
    fn test_parse_scheme() {
        assert_eq!(parse_scheme("https://google.com"), Some("https"));
        assert_eq!(parse_scheme("http://google.com"), Some("http"));
        assert_eq!(parse_scheme("google.com"), None);
        assert_eq!(parse_scheme(""), None);
    }
}
