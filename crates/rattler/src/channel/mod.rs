use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use thiserror::Error;
use url::Url;

pub use fetch::{FetchRepoDataError, FetchRepoDataProgress};

use super::{ParsePlatformError, Platform};

mod fetch;

/// The `ChannelConfig` describes properties that are required to resolve "simple" channel names to
/// channel URLs.
///
/// When working with [`Channel`]s you want to resolve them to a Url. The Url describes where to
/// find the data in the channel. Working with URLs is less user friendly since most of the time
/// users only use channels from one particular server. Conda solves this by allowing users not to
/// specify a full Url but instead only specify the name of the channel and reading the primary
/// server address from a configuration file (e.g. `.condarc`).
#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct ChannelConfig {
    /// A url to prefix to channel names that don't start with a Url. Usually this Url refers to
    /// the `https://conda.anaconda.org` server but users are free to change this. This allows
    /// naming channels just by their name instead of their entire Url (e.g. "conda-forge" actually
    /// refers to "https://conda.anaconda.org/conda-forge").
    ///
    /// The default value is: https://conda.anaconda.org
    pub channel_alias: Url,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        ChannelConfig {
            channel_alias: Url::from_str("https://conda.anaconda.org")
                .expect("could not parse default channel alias"),
        }
    }
}

/// `Channel`s are the primary source of package information.
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct Channel {
    /// The platforms supported by this channel, or None if no explicit platforms have been
    /// specified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platforms: Option<SmallVec<[Platform; 2]>>,

    /// The Url scheme of the channel. Usually http, https or file.
    pub scheme: String,

    /// The server and path part of the Url.
    pub location: String,

    /// The name of the channel
    pub name: Option<String>,
}

impl Channel {
    /// Parses a [`Channel`] from a string and a channel configuration.
    pub fn from_str(
        str: impl AsRef<str>,
        config: &ChannelConfig,
    ) -> Result<Self, ParseChannelError> {
        let str = str.as_ref();
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

    /// Constructs a new [`Channel`] from a `Url` and associated platforms.
    pub fn from_url(
        url: Url,
        platforms: Option<impl Into<SmallVec<[Platform; 2]>>>,
        _config: &ChannelConfig,
    ) -> Self {
        let path = url.path().trim_end_matches('/');

        // Case 1: No path give, channel name is ""

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
            let name = path.trim_start_matches('/');
            Self {
                platforms: platforms.map(Into::into),
                scheme: url.scheme().to_owned(),
                location,
                name: (!name.is_empty()).then(|| name).map(str::to_owned),
            }
        } else {
            // Case 6: non-otherwise-specified file://-type urls
            let (location, name) = url
                .path()
                .rsplit_once('/')
                .unwrap_or_else(|| ("/", url.path()));
            Self {
                platforms: platforms.map(Into::into),
                scheme: String::from("file"),
                location: location.to_owned(),
                name: (!name.is_empty()).then(|| name).map(str::to_owned),
            }
        }
    }

    /// Construct a channel from a name, platform and configuration.
    pub fn from_name(
        name: &str,
        platforms: Option<impl Into<SmallVec<[Platform; 2]>>>,
        config: &ChannelConfig,
    ) -> Self {
        // TODO: custom channels
        Self {
            platforms: platforms.map(Into::into),
            scheme: config.channel_alias.scheme().to_owned(),
            location: format!(
                "{}/{}",
                config.channel_alias.host_str().unwrap_or("/").to_owned(),
                config.channel_alias.path()
            )
            .trim_end_matches('/')
            .to_owned(),
            name: (!name.is_empty()).then(|| name).map(str::to_owned),
        }
    }

    /// Returns the base Url of the channel. This does not include the platform part.
    pub fn base_url(&self) -> Url {
        let url = Url::from_str(&format!("{}://{}", self.scheme, self.location,))
            .expect("could not construct base_url for channel");

        if let Some(name) = &self.name {
            url.join(&format!("{name}/"))
                .expect("name must be a valid Url path")
        } else {
            url
        }
    }

    /// Returns the Urls for the given platform
    pub fn platform_url(&self, platform: Platform) -> Url {
        self.base_url()
            .join(&format!("{}/", platform.as_str())) // trailing slash is important here as this signifies a directory
            .expect("platform is a valid url fragment")
    }

    /// Returns the Urls for all the supported platforms of this package.
    pub fn platforms_url(&self) -> Vec<(Platform, Url)> {
        self.platforms_or_default()
            .iter()
            .map(|&platform| (platform, self.platform_url(platform)))
            .collect()
    }

    /// Returns the platforms explicitly mentioned in the channel or the default platforms of the
    /// current system.
    pub fn platforms_or_default(&self) -> &[Platform] {
        if let Some(platforms) = &self.platforms {
            platforms.as_slice()
        } else {
            default_platforms()
        }
    }

    /// Returns the canonical name of the channel
    pub fn canonical_name(&self) -> String {
        let result = format!("{}://{}", self.scheme, self.location);
        if let Some(name) = &self.name {
            format!("{result}/{name}")
        } else {
            result
        }
    }
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
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
#[allow(clippy::type_complexity)]
fn parse_platforms(
    channel: &str,
) -> Result<(Option<SmallVec<[Platform; 2]>>, &str), ParsePlatformError> {
    if channel.rfind(']').is_some() {
        if let Some(start_platform_idx) = channel.find('[') {
            let platform_part = &channel[start_platform_idx + 1..channel.len() - 1];
            let platforms = platform_part
                .split(',')
                .map(str::trim)
                .map(FromStr::from_str)
                .collect::<Result<_, _>>()?;
            return Ok((Some(platforms), &channel[0..start_platform_idx]));
        }
    }

    Ok((None, channel))
}

/// Returns the default platforms. These are based on the platform this binary was build for as well
/// as platform agnostic platforms.
pub const fn default_platforms() -> &'static [Platform] {
    const CURRENT_PLATFORMS: [Platform; 2] = [Platform::current(), Platform::NoArch];
    &CURRENT_PLATFORMS
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
    use smallvec::smallvec;

    use super::{parse_scheme, Channel, ChannelConfig, Platform};

    #[test]
    fn test_parse_scheme() {
        assert_eq!(parse_scheme("https://google.com"), Some("https"));
        assert_eq!(parse_scheme("http://google.com"), Some("http"));
        assert_eq!(parse_scheme("google.com"), None);
        assert_eq!(parse_scheme(""), None);
    }

    #[test]
    fn parse_by_name() {
        let config = ChannelConfig::default();

        let channel = Channel::from_str("conda-forge", &config).unwrap();
        assert_eq!(channel.scheme, "https");
        assert_eq!(channel.location, "conda.anaconda.org");
        assert_eq!(channel.name.as_deref(), Some("conda-forge"));
        assert_eq!(channel.platforms, None);
        assert_eq!(
            channel.base_url().to_string(),
            "https://conda.anaconda.org/conda-forge/"
        );
    }

    #[test]
    fn parse_url_only() {
        let config = ChannelConfig::default();

        let channel = Channel::from_str("http://localhost:1234", &config).unwrap();
        assert_eq!(channel.scheme, "http");
        assert_eq!(channel.location, "localhost:1234");
        assert_eq!(channel.name, None);
        assert_eq!(channel.platforms, None);

        let noarch_url = channel.platform_url(Platform::NoArch);
        assert_eq!(noarch_url.to_string(), "http://localhost:1234/noarch/");
    }

    #[test]
    fn parse_platform() {
        let platform = Platform::Linux32;
        let config = ChannelConfig::default();

        let channel = Channel::from_str(
            format!("https://conda.anaconda.com/conda-forge[{platform}]"),
            &config,
        )
        .unwrap();
        assert_eq!(channel.scheme, "https");
        assert_eq!(channel.location, "conda.anaconda.com");
        assert_eq!(channel.name.as_deref(), Some("conda-forge"));
        assert_eq!(channel.platforms, Some(smallvec![platform]));

        let channel = Channel::from_str(
            format!("https://repo.anaconda.com/pkgs/main[{platform}]"),
            &config,
        )
        .unwrap();
        assert_eq!(channel.scheme, "https");
        assert_eq!(channel.location, "repo.anaconda.com");
        assert_eq!(channel.name.as_deref(), Some("pkgs/main"));
        assert_eq!(channel.platforms, Some(smallvec![platform]));
    }
}
