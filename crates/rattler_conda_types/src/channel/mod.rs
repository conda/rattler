use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

use file_url::directory_path_to_url;
use rattler_redaction::Redact;
use serde::{Deserialize, Serialize, Serializer};
use thiserror::Error;
use typed_path::{Utf8NativePathBuf, Utf8TypedPath, Utf8TypedPathBuf};
use url::Url;

use super::{ParsePlatformError, Platform};
use crate::utils::{
    path::is_path,
    url::{add_trailing_slash, parse_scheme},
};

const DEFAULT_CHANNEL_ALIAS: &str = "https://conda.anaconda.org";

/// The `ChannelConfig` describes properties that are required to resolve
/// "simple" channel names to channel URLs.
///
/// When working with [`Channel`]s you want to resolve them to a Url. The Url
/// describes where to find the data in the channel. Working with URLs is less
/// user friendly since most of the time users only use channels from one
/// particular server. Conda solves this by allowing users not to specify a full
/// Url but instead only specify the name of the channel and reading the primary
/// server address from a configuration file (e.g. `.condarc`).
#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct ChannelConfig {
    /// A url to prefix to channel names that don't start with a Url. Usually
    /// this Url refers to the `https://conda.anaconda.org` server but users are free to change this. This allows
    /// naming channels just by their name instead of their entire Url (e.g.
    /// "conda-forge" actually refers to `<https://conda.anaconda.org/conda-forge>`).
    ///
    /// The default value is: <https://conda.anaconda.org>
    pub channel_alias: Url,

    /// For local channels, the root directory from which to resolve relative
    /// paths. Most of the time you would initialize this with the current
    /// working directory.
    pub root_dir: PathBuf,
}

impl ChannelConfig {
    /// Create a new `ChannelConfig` with the default values.
    pub fn default_with_root_dir(root_dir: PathBuf) -> Self {
        Self {
            root_dir,
            channel_alias: Url::from_str(DEFAULT_CHANNEL_ALIAS)
                .expect("could not parse default channel alias"),
        }
    }

    /// Strip the channel alias if the base url is "under" the channel alias.
    /// This returns the name of the channel (for example "conda-forge" for
    /// `https://conda.anaconda.org/conda-forge` when the channel alias is
    /// `https://conda.anaconda.org`).
    pub fn strip_channel_alias(&self, base_url: &Url) -> Option<String> {
        base_url
            .as_str()
            .strip_prefix(self.channel_alias.as_str())
            .map(|s| s.trim_end_matches('/').to_string())
    }

    /// Returns the canonical name of a channel with the given base url.
    pub fn canonical_name(&self, base_url: &Url) -> String {
        if let Some(stripped) = base_url.as_str().strip_prefix(self.channel_alias.as_str()) {
            stripped.trim_end_matches('/').to_string()
        } else {
            base_url.clone().redact().to_string()
        }
    }
}

/// Represents a channel description as either a name (e.g. `conda-forge`) or a
/// base url.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum NamedChannelOrUrl {
    /// A named channel
    Name(String),

    /// A url
    Url(Url),

    /// A path. Can either be absolute or relative.
    Path(Utf8TypedPathBuf),
}

impl NamedChannelOrUrl {
    /// Returns the string representation of the channel.
    ///
    /// This method ensures that if the channel is a url, it does not end with a
    /// `/`.
    pub fn as_str(&self) -> &str {
        match self {
            NamedChannelOrUrl::Name(name) => name,
            NamedChannelOrUrl::Url(url) => url.as_str().trim_end_matches('/'),
            NamedChannelOrUrl::Path(path) => path.as_str(),
        }
    }

    /// Converts the channel to a base url using the given configuration.
    /// This method ensures that the base url always ends with a `/`.
    pub fn into_base_url(self, config: &ChannelConfig) -> Result<Url, ParseChannelError> {
        let url = match self {
            NamedChannelOrUrl::Name(name) => {
                let mut base_url = config.channel_alias.clone();
                if let Ok(mut segments) = base_url.path_segments_mut() {
                    for segment in name.split(&['/', '\\']) {
                        segments.push(segment);
                    }
                }
                base_url
            }
            NamedChannelOrUrl::Url(url) => url,
            NamedChannelOrUrl::Path(path) => {
                let absolute_path = absolute_path(path.as_str(), &config.root_dir)?;
                directory_path_to_url(absolute_path.to_path())
                    .map_err(|_err| ParseChannelError::InvalidPath(path.to_string()))?
            }
        };
        Ok(add_trailing_slash(&url).into_owned())
    }

    /// Converts this instance into a channel.
    pub fn into_channel(self, config: &ChannelConfig) -> Result<Channel, ParseChannelError> {
        let name = match &self {
            NamedChannelOrUrl::Name(name) => Some(name.clone()),
            NamedChannelOrUrl::Url(base_url) => config.strip_channel_alias(base_url),
            NamedChannelOrUrl::Path(_) => None,
        };
        let base_url = self.into_base_url(config)?;
        Ok(Channel {
            name,
            ..Channel::from_url(base_url)
        })
    }
}

impl Display for NamedChannelOrUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for NamedChannelOrUrl {
    type Err = url::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if parse_scheme(s).is_some() {
            Ok(NamedChannelOrUrl::Url(Url::from_str(s)?))
        } else if is_path(s) {
            Ok(NamedChannelOrUrl::Path(s.into()))
        } else {
            Ok(NamedChannelOrUrl::Name(s.to_string()))
        }
    }
}

impl<'de> serde::Deserialize<'de> for NamedChannelOrUrl {
    fn deserialize<D>(deserializer: D) -> Result<NamedChannelOrUrl, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        NamedChannelOrUrl::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for NamedChannelOrUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_str().serialize(serializer)
    }
}

/// `Channel`s are the primary source of package information.
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct Channel {
    /// The platforms supported by this channel, or None if no explicit
    /// platforms have been specified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<Platform>>,

    /// Base URL of the channel, everything is relative to this url.
    pub base_url: Url,

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
            Channel {
                platforms,
                ..Channel::from_url(url)
            }
        } else if is_path(channel) {
            #[cfg(target_arch = "wasm32")]
            return Err(ParseChannelError::InvalidPath(path));

            #[cfg(not(target_arch = "wasm32"))]
            {
                let absolute_path = absolute_path(channel, &config.root_dir)?;
                let url = directory_path_to_url(absolute_path.to_path())
                    .map_err(|_err| ParseChannelError::InvalidPath(channel.to_owned()))?;
                Self {
                    platforms,
                    base_url: url,
                    name: Some(channel.to_owned()),
                }
            }
        } else {
            // Validate that the channel is a valid name
            if channel.contains([':', '\\']) {
                return Err(ParseChannelError::InvalidName(channel.to_owned()));
            }
            Channel {
                platforms,
                ..Channel::from_name(channel, config)
            }
        };

        Ok(channel)
    }

    /// Set the explicit platforms of the channel.
    pub fn with_explicit_platforms(self, platforms: impl IntoIterator<Item = Platform>) -> Self {
        Self {
            platforms: Some(platforms.into_iter().collect()),
            ..self
        }
    }

    /// Constructs a new [`Channel`] from a `Url` and associated platforms.
    pub fn from_url(url: Url) -> Self {
        // Get the path part of the URL but trim the directory suffix
        let path = url.path().trim_end_matches('/');

        // Ensure that the base_url does always ends in a `/`
        let base_url = if url.path().ends_with('/') {
            url.clone()
        } else {
            let mut url = url.clone();
            url.set_path(&format!("{path}/"));
            url
        };

        // Case 1: No path give, channel name is ""

        // Case 2: migrated_custom_channels
        // Case 3: migrated_channel_aliases
        // Case 4: custom_channels matches
        // Case 5: channel_alias match

        if base_url.has_host() {
            // Case 7: Fallback
            let name = path.trim_start_matches('/');
            Self {
                platforms: None,
                name: (!name.is_empty()).then_some(name).map(str::to_owned),
                base_url,
            }
        } else {
            // Case 6: non-otherwise-specified file://-type urls
            let name = path
                .rsplit_once('/')
                .map_or_else(|| base_url.path(), |(_, path_part)| path_part);
            Self {
                platforms: None,
                name: (!name.is_empty()).then_some(name).map(str::to_owned),
                base_url,
            }
        }
    }

    /// Construct a channel from a name, platform and configuration.
    pub fn from_name(name: &str, config: &ChannelConfig) -> Self {
        // TODO: custom channels

        let dir_name = if name.ends_with('/') {
            Cow::Borrowed(name)
        } else {
            Cow::Owned(format!("{name}/"))
        };

        let name = name.trim_end_matches('/');
        Self {
            platforms: None,
            base_url: config
                .channel_alias
                .join(dir_name.as_ref())
                .expect("name is not a valid Url"),
            name: (!name.is_empty()).then_some(name).map(str::to_owned),
        }
    }

    /// Constructs a channel from a directory path.
    ///
    /// # Panics
    ///
    /// Panics if the path is not an absolute path or could not be
    /// canonicalized.
    pub fn from_directory(path: &Path) -> Self {
        let path = if path.is_absolute() {
            Cow::Borrowed(path)
        } else {
            Cow::Owned(
                path.canonicalize()
                    .expect("path is a not a valid absolute path"),
            )
        };

        let url = Url::from_directory_path(path).expect("path is a valid url");
        Self {
            platforms: None,
            base_url: url,
            name: None,
        }
    }

    /// Returns the name of the channel
    pub fn name(&self) -> &str {
        match self.base_url().scheme() {
            // The name of the channel is only defined for http and https channels.
            // If the name is not defined we return the base url.
            "https" | "http" => self
                .name
                .as_deref()
                .unwrap_or_else(|| self.base_url.as_str()),
            _ => self.base_url.as_str(),
        }
    }

    /// Returns the base Url of the channel. This does not include the platform
    /// part.
    pub fn base_url(&self) -> &Url {
        &self.base_url
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

    /// Returns the platforms explicitly mentioned in the channel or the default
    /// platforms of the current system.
    pub fn platforms_or_default(&self) -> &[Platform] {
        if let Some(platforms) = &self.platforms {
            platforms.as_slice()
        } else {
            default_platforms()
        }
    }

    /// Returns the canonical name of the channel
    pub fn canonical_name(&self) -> String {
        self.base_url.clone().redact().to_string()
    }
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
/// Error that can occur when parsing a channel.
pub enum ParseChannelError {
    /// Error when the platform could not be parsed.
    #[error("could not parse the platforms")]
    ParsePlatformError(#[source] ParsePlatformError),

    /// Error when the url could not be parsed.
    #[error("could not parse url")]
    ParseUrlError(#[source] url::ParseError),

    /// Error when the path is invalid.
    #[error("invalid path '{0}'")]
    InvalidPath(String),

    /// Error when the channel name is invalid.
    #[error("invalid channel name: '{0}'")]
    InvalidName(String),

    /// The root directory is not an absolute path
    #[error("root directory: '{0}' from channel config is not an absolute path")]
    NonAbsoluteRootDir(PathBuf),

    /// The root directory is not UTF-8 encoded.
    #[error("root directory: '{0}' of channel config is not utf8 encoded")]
    NotUtf8RootDir(PathBuf),
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
fn parse_platforms(channel: &str) -> Result<(Option<Vec<Platform>>, &str), ParsePlatformError> {
    if channel.rfind(']').is_some() {
        if let Some(start_platform_idx) = channel.find('[') {
            let platform_part = &channel[start_platform_idx + 1..channel.len() - 1];
            let platforms = platform_part
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(FromStr::from_str)
                .collect::<Result<Vec<_>, _>>()?;
            let platforms = if platforms.is_empty() {
                None
            } else {
                Some(platforms)
            };
            return Ok((platforms, &channel[0..start_platform_idx]));
        }
    }

    Ok((None, channel))
}

/// Returns the default platforms. These are based on the platform this binary
/// was build for as well as platform agnostic platforms.
pub(crate) const fn default_platforms() -> &'static [Platform] {
    const CURRENT_PLATFORMS: [Platform; 2] = [Platform::current(), Platform::NoArch];
    &CURRENT_PLATFORMS
}

/// Returns the specified path as an absolute path
fn absolute_path(path_str: &str, root_dir: &Path) -> Result<Utf8TypedPathBuf, ParseChannelError> {
    let path = Utf8TypedPath::from(path_str);
    if path.is_absolute() {
        return Ok(path.normalize());
    }

    // Parse the `~/` as the home folder
    if let Ok(user_path) = path.strip_prefix("~/") {
        return Ok(Utf8TypedPathBuf::from(
            dirs::home_dir()
                .ok_or(ParseChannelError::InvalidPath(path.to_string()))?
                .to_str()
                .ok_or(ParseChannelError::NotUtf8RootDir(PathBuf::from(path_str)))?,
        )
        .join(user_path)
        .normalize());
    }

    let root_dir_str = root_dir
        .to_str()
        .ok_or_else(|| ParseChannelError::NotUtf8RootDir(root_dir.to_path_buf()))?;
    let native_root_dir = Utf8NativePathBuf::from(root_dir_str);

    if !native_root_dir.is_absolute() {
        return Err(ParseChannelError::NonAbsoluteRootDir(
            root_dir.to_path_buf(),
        ));
    }

    Ok(native_root_dir.to_typed_path().join(path).normalize())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use typed_path::{NativePath, Utf8NativePath};
    use url::Url;

    use super::*;

    #[test]
    fn test_parse_platforms() {
        assert_eq!(
            parse_platforms("[noarch, linux-64]"),
            Ok((Some(vec![Platform::NoArch, Platform::Linux64]), ""))
        );
        assert_eq!(
            parse_platforms("sometext[noarch]"),
            Ok((Some(vec![Platform::NoArch]), "sometext"))
        );
        assert_eq!(
            parse_platforms("sometext[noarch,]"),
            Ok((Some(vec![Platform::NoArch]), "sometext"))
        );
        assert_eq!(parse_platforms("sometext[]"), Ok((None, "sometext")));
        assert!(matches!(
            parse_platforms("[notaplatform]"),
            Err(ParsePlatformError { .. })
        ));
    }

    #[test]
    fn test_absolute_path() {
        let current_dir = std::env::current_dir().expect("no current dir?");
        let native_current_dir = typed_path::utils::utf8_current_dir()
            .expect("")
            .to_typed_path_buf();
        assert_eq!(
            absolute_path(".", &current_dir).as_ref(),
            Ok(&native_current_dir)
        );
        assert_eq!(
            absolute_path("foo", &current_dir).as_ref(),
            Ok(&native_current_dir.join("foo"))
        );

        let mut parent_dir = native_current_dir.clone();
        assert!(parent_dir.pop());

        assert_eq!(absolute_path("..", &current_dir).as_ref(), Ok(&parent_dir));
        assert_eq!(
            absolute_path("../foo", &current_dir).as_ref(),
            Ok(&parent_dir.join("foo"))
        );

        let home_dir = dirs::home_dir()
            .unwrap()
            .into_os_string()
            .into_encoded_bytes();
        let home_dir = Utf8NativePath::from_bytes_path(NativePath::new(&home_dir))
            .unwrap()
            .to_typed_path();
        assert_eq!(
            absolute_path("~/unix_dir", &current_dir).unwrap(),
            home_dir.join("unix_dir")
        );
        assert_eq!(
            absolute_path("~/unix_dir/test/../test2", &current_dir).unwrap(),
            home_dir.join("unix_dir").join("test2")
        );
    }

    #[test]
    fn test_parse_scheme() {
        assert_eq!(parse_scheme("https://google.com"), Some("https"));
        assert_eq!(parse_scheme("http://google.com"), Some("http"));
        assert_eq!(parse_scheme("google.com"), None);
        assert_eq!(parse_scheme(""), None);
        assert_eq!(parse_scheme("waytoolongscheme://"), None);
        assert_eq!(parse_scheme("1nv4l1d://"), None);
        assert_eq!(parse_scheme("sch3m3://"), Some("sch3m3"));
        assert_eq!(parse_scheme("scheme://"), Some("scheme"));
        assert_eq!(parse_scheme("$ch3m3://"), None);
        assert_eq!(parse_scheme("sch#me://"), None);
    }

    #[test]
    fn parse_by_name() {
        let config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());

        let channel = Channel::from_str("conda-forge", &config).unwrap();
        assert_eq!(
            channel.base_url,
            Url::from_str("https://conda.anaconda.org/conda-forge/").unwrap()
        );
        assert_eq!(channel.name.as_deref(), Some("conda-forge"));
        assert_eq!(channel.name(), "conda-forge");
        assert_eq!(channel.platforms, None);

        assert_eq!(channel, Channel::from_name("conda-forge/", &config));
    }

    #[test]
    fn parse_from_url() {
        let config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());

        let channel =
            Channel::from_str("https://conda.anaconda.org/conda-forge/", &config).unwrap();
        assert_eq!(
            channel.base_url,
            Url::from_str("https://conda.anaconda.org/conda-forge/").unwrap()
        );
        assert_eq!(channel.name.as_deref(), Some("conda-forge"));
        assert_eq!(channel.name(), "conda-forge");
        assert_eq!(channel.platforms, None);
        assert_eq!(
            channel.base_url().to_string(),
            "https://conda.anaconda.org/conda-forge/"
        );

        let channel = Channel::from_str(
            "https://conda.anaconda.org/conda-forge/label/rust_dev",
            &config,
        );
        assert_eq!(channel.unwrap().name(), "conda-forge/label/rust_dev",);
    }

    #[test]
    fn parse_from_file_path() {
        let config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());

        let channel = Channel::from_str("file:///var/channels/conda-forge", &config).unwrap();
        assert_eq!(channel.name.as_deref(), Some("conda-forge"));
        assert_eq!(channel.name(), "file:///var/channels/conda-forge/");
        assert_eq!(
            channel.base_url,
            Url::from_str("file:///var/channels/conda-forge/").unwrap()
        );
        assert_eq!(channel.platforms, None);
        assert_eq!(
            channel.base_url().to_string(),
            "file:///var/channels/conda-forge/"
        );

        let current_dir = std::env::current_dir().expect("no current dir?");
        let channel = Channel::from_str("./dir/does/not_exist", &config).unwrap();
        assert_eq!(channel.name.as_deref(), Some("./dir/does/not_exist"));
        let expected = absolute_path("./dir/does/not_exist", &current_dir).unwrap();
        assert_eq!(
            channel.name(),
            file_url::directory_path_to_url(expected.to_path())
                .unwrap()
                .as_str()
        );
        assert_eq!(channel.platforms, None);
        assert_eq!(
            channel.base_url().to_file_path().unwrap(),
            current_dir.join("dir/does/not_exist")
        );
    }

    #[test]
    fn parse_url_only() {
        let config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());

        let channel = Channel::from_str("http://localhost:1234", &config).unwrap();
        assert_eq!(
            channel.base_url,
            Url::from_str("http://localhost:1234/").unwrap()
        );
        assert_eq!(channel.name, None);
        assert_eq!(channel.platforms, None);
        assert_eq!(channel.name(), "http://localhost:1234/");

        let noarch_url = channel.platform_url(Platform::NoArch);
        assert_eq!(noarch_url.to_string(), "http://localhost:1234/noarch/");

        assert!(matches!(
            Channel::from_str("http://1000.0000.0001.294", &config),
            Err(ParseChannelError::ParseUrlError(_))
        ));
    }

    #[test]
    fn parse_platform() {
        let platform = Platform::Linux32;
        let config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());

        let channel = Channel::from_str(
            format!("https://conda.anaconda.org/conda-forge[{platform}]"),
            &config,
        )
        .unwrap();
        assert_eq!(
            channel.base_url,
            Url::from_str("https://conda.anaconda.org/conda-forge/").unwrap()
        );
        assert_eq!(channel.name.as_deref(), Some("conda-forge"));
        assert_eq!(channel.platforms, Some(vec![platform]));

        let channel = Channel::from_str(
            format!("https://conda.anaconda.org/pkgs/main[{platform}]"),
            &config,
        )
        .unwrap();
        assert_eq!(
            channel.base_url,
            Url::from_str("https://conda.anaconda.org/pkgs/main/").unwrap()
        );
        assert_eq!(channel.name.as_deref(), Some("pkgs/main"));
        assert_eq!(channel.platforms, Some(vec![platform]));

        let channel = Channel::from_str("conda-forge/label/rust_dev", &config).unwrap();
        assert_eq!(
            channel.base_url,
            Url::from_str("https://conda.anaconda.org/conda-forge/label/rust_dev/").unwrap()
        );
        assert_eq!(channel.name.as_deref(), Some("conda-forge/label/rust_dev"));
    }

    #[test]
    fn channel_canonical_name() {
        let config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let channel = Channel::from_str("http://localhost:1234", &config).unwrap();

        assert_eq!(channel.canonical_name(), "http://localhost:1234/");

        let channel = Channel::from_str("http://user:password@localhost:1234", &config).unwrap();

        assert_eq!(
            channel.canonical_name(),
            "http://user:********@localhost:1234/"
        );

        let channel =
            Channel::from_str("http://localhost:1234/t/secretfoo/blablub", &config).unwrap();
        assert_eq!(
            channel.canonical_name(),
            "http://localhost:1234/t/********/blablub/"
        );
    }

    #[test]
    fn config_canonical_name() {
        let channel_config = ChannelConfig {
            channel_alias: Url::from_str("https://conda.anaconda.org").unwrap(),
            root_dir: std::env::current_dir().expect("No current dir set"),
        };
        assert_eq!(
            channel_config
                .canonical_name(&Url::from_str("https://conda.anaconda.org/conda-forge/").unwrap())
                .as_str(),
            "conda-forge"
        );
        assert_eq!(
            channel_config
                .canonical_name(&Url::from_str("https://prefix.dev/conda-forge/").unwrap())
                .as_str(),
            "https://prefix.dev/conda-forge/"
        );
        assert_eq!(
            channel_config
                .canonical_name(
                    &Url::from_str("https://prefix.dev/t/mysecrettoken/conda-forge/").unwrap()
                )
                .as_str(),
            "https://prefix.dev/t/********/conda-forge/"
        );

        assert_eq!(
            channel_config
                .canonical_name(
                    &Url::from_str("https://user:secret@prefix.dev/conda-forge/").unwrap()
                )
                .as_str(),
            "https://user:********@prefix.dev/conda-forge/"
        );
    }

    #[test]
    fn compare_channel_with_or_without_backslash() {
        let channel_config = ChannelConfig {
            channel_alias: Url::from_str("https://conda.anaconda.org").unwrap(),
            root_dir: std::env::current_dir().expect("No current dir set"),
        };

        // Normal channel should have backslash
        let test_channels = vec![
            "conda-forge",
            "conda-forge/",
            "https://conda.anaconda.org/conda-forge",
            "https://conda.anaconda.org/conda-forge/",
            "../conda-forge/",
            "../conda-forge",
        ];

        for channel_str in test_channels {
            let channel = Channel::from_str(channel_str, &channel_config).unwrap();
            assert!(channel.base_url().as_str().ends_with('/'));
            assert!(!channel.base_url().as_str().ends_with("//"));

            let named_channel = NamedChannelOrUrl::from_str(channel_str).unwrap();
            let base_url = named_channel
                .clone()
                .into_base_url(&channel_config)
                .unwrap();
            let base_url_str = base_url.as_str();
            assert!(base_url_str.ends_with('/'));
            assert!(!base_url_str.ends_with("//"));

            let channel = named_channel.into_channel(&channel_config).unwrap();
            assert!(channel.base_url().as_str().ends_with('/'));
            assert!(!channel.base_url().as_str().ends_with("//"));
        }
    }

    #[test]
    fn test_compare_channel_and_named_channel_or_url() {
        let channel_config = ChannelConfig {
            channel_alias: Url::from_str("https://conda.anaconda.org").unwrap(),
            root_dir: std::env::current_dir().expect("No current dir set"),
        };
        let named = NamedChannelOrUrl::Name("conda-forge".to_string());
        let channel = Channel::from_str("conda-forge", &channel_config).unwrap();
        assert_eq!(
            &channel.base_url,
            named.into_channel(&channel_config).unwrap().base_url()
        );

        let named = NamedChannelOrUrl::Name("nvidia/label/cuda-11.8.0".to_string());
        let channel = Channel::from_str("nvidia/label/cuda-11.8.0", &channel_config).unwrap();
        assert_eq!(
            channel.base_url(),
            named.into_channel(&channel_config).unwrap().base_url()
        );
    }
}
