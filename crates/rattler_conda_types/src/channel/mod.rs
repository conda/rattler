use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use thiserror::Error;
use url::Url;

use super::{ParsePlatformError, Platform};

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
    /// refers to `<https://conda.anaconda.org/conda-forge>`).
    ///
    /// The default value is: <https://conda.anaconda.org>
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
            Channel::from_url(url, platforms, config)
        } else if is_path(channel) {
            let path = PathBuf::from(channel);

            #[cfg(target_arch = "wasm32")]
            return Err(ParseChannelError::InvalidPath(path));

            #[cfg(not(target_arch = "wasm32"))]
            {
                let absolute_path = absolute_path(&path);
                let url = Url::from_directory_path(absolute_path)
                    .map_err(|_| ParseChannelError::InvalidPath(path))?;
                Self {
                    platforms,
                    base_url: url,
                    name: Some(channel.to_owned()),
                }
            }
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
        // Get the path part of the URL but trim the directory suffix
        let path = url.path().trim_end_matches('/');

        // Ensure that the base_url does always ends in a `/`
        let base_url = if !url.path().ends_with('/') {
            let mut url = url.clone();
            url.set_path(&format!("{path}/"));
            url
        } else {
            url.clone()
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
                platforms: platforms.map(Into::into),
                name: (!name.is_empty()).then_some(name).map(str::to_owned),
                base_url,
            }
        } else {
            // Case 6: non-otherwise-specified file://-type urls
            let name = path
                .rsplit_once('/')
                .map(|(_, path_part)| path_part)
                .unwrap_or_else(|| base_url.path());
            Self {
                platforms: platforms.map(Into::into),
                name: (!name.is_empty()).then_some(name).map(str::to_owned),
                base_url,
            }
        }
    }

    /// Construct a channel from a name, platform and configuration.
    pub fn from_name(
        name: &str,
        platforms: Option<SmallVec<[Platform; 2]>>,
        config: &ChannelConfig,
    ) -> Self {
        // TODO: custom channels

        let dir_name = if !name.ends_with('/') {
            Cow::Owned(format!("{name}/"))
        } else {
            Cow::Borrowed(name)
        };

        let name = name.trim_end_matches('/');
        Self {
            platforms,
            base_url: config
                .channel_alias
                .join(dir_name.as_ref())
                .expect("name is not a valid Url"),
            name: (!name.is_empty()).then_some(name).map(str::to_owned),
        }
    }

    /// Returns the base Url of the channel. This does not include the platform part.
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
        self.base_url.to_string()
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
            let platforms: SmallVec<_> = platform_part
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(FromStr::from_str)
                .collect::<Result<_, _>>()?;
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

/// Returns the default platforms. These are based on the platform this binary was build for as well
/// as platform agnostic platforms.
pub(crate) const fn default_platforms() -> &'static [Platform] {
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
    lazy_regex::regex!(r"(\./|\.\.|~|/|[a-zA-Z]:[/\\]|\\\\|//)").is_match(path)
}

/// Normalizes a file path by eliminating `..` and `.`.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = path.components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().cloned() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };

    for component in components {
        match component {
            Component::Prefix(..) => unreachable!(),
            Component::RootDir => {
                ret.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                ret.pop();
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }
    ret
}

/// Returns the specified path as an absolute path
fn absolute_path(path: &Path) -> Cow<'_, Path> {
    if path.is_absolute() {
        return Cow::Borrowed(path);
    }

    let current_dir = std::env::current_dir().expect("missing current directory?");
    let absolute_dir = current_dir.join(path);
    Cow::Owned(normalize_path(&absolute_dir))
}

#[cfg(test)]
mod tests {
    use crate::channel::{absolute_path, normalize_path, parse_platforms};
    use crate::{ParseChannelError, ParsePlatformError};
    use smallvec::smallvec;
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use url::Url;

    use super::{parse_scheme, Channel, ChannelConfig, Platform};

    #[test]
    fn test_parse_platforms() {
        assert_eq!(
            parse_platforms("[noarch, linux-64]"),
            Ok((Some(smallvec![Platform::NoArch, Platform::Linux64]), ""))
        );
        assert_eq!(
            parse_platforms("sometext[noarch]"),
            Ok((Some(smallvec![Platform::NoArch]), "sometext"))
        );
        assert_eq!(
            parse_platforms("sometext[noarch,]"),
            Ok((Some(smallvec![Platform::NoArch]), "sometext"))
        );
        assert_eq!(parse_platforms("sometext[]"), Ok((None, "sometext")));
        assert!(matches!(
            parse_platforms("[notaplatform]"),
            Err(ParsePlatformError { .. })
        ));
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(
            normalize_path(Path::new("foo/bar")),
            PathBuf::from("foo/bar")
        );
        assert_eq!(
            normalize_path(Path::new("foo/bar/")),
            PathBuf::from("foo/bar/")
        );
        assert_eq!(
            normalize_path(Path::new("./foo/bar")),
            PathBuf::from("foo/bar")
        );
        assert_eq!(
            normalize_path(Path::new("./foo/../bar")),
            PathBuf::from("bar")
        );
        assert_eq!(
            normalize_path(Path::new("./foo/../bar/..")),
            PathBuf::from("")
        );
    }

    #[test]
    fn test_absolute_path() {
        let current_dir = std::env::current_dir().expect("no current dir?");
        assert_eq!(absolute_path(Path::new(".")).as_ref(), &current_dir);
        assert_eq!(absolute_path(Path::new(".")).as_ref(), &current_dir);
        assert_eq!(
            absolute_path(Path::new("foo")).as_ref(),
            &current_dir.join("foo")
        );

        let mut parent_dir = current_dir;
        assert!(parent_dir.pop());

        assert_eq!(absolute_path(Path::new("..")).as_ref(), &parent_dir);
        assert_eq!(
            absolute_path(Path::new("../foo")).as_ref(),
            &parent_dir.join("foo")
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
        let config = ChannelConfig::default();

        let channel = Channel::from_str("conda-forge", &config).unwrap();
        assert_eq!(
            channel.base_url,
            Url::from_str("https://conda.anaconda.org/conda-forge/").unwrap()
        );
        assert_eq!(channel.name.as_deref(), Some("conda-forge"));
        assert_eq!(channel.platforms, None);

        assert_eq!(channel, Channel::from_name("conda-forge/", None, &config));
    }

    #[test]
    fn parse_from_url() {
        let config = ChannelConfig::default();

        let channel =
            Channel::from_str("https://conda.anaconda.org/conda-forge/", &config).unwrap();
        assert_eq!(
            channel.base_url,
            Url::from_str("https://conda.anaconda.org/conda-forge/").unwrap()
        );
        assert_eq!(channel.name.as_deref(), Some("conda-forge"));
        assert_eq!(channel.platforms, None);
        assert_eq!(
            channel.base_url().to_string(),
            "https://conda.anaconda.org/conda-forge/"
        );
    }

    #[test]
    fn parse_from_file_path() {
        let config = ChannelConfig::default();

        let channel = Channel::from_str("file:///var/channels/conda-forge", &config).unwrap();
        assert_eq!(channel.name.as_deref(), Some("conda-forge"));
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
        assert_eq!(channel.platforms, None);
        assert_eq!(
            channel.base_url().to_file_path().unwrap(),
            current_dir.join("dir/does/not_exist")
        );
    }

    #[test]
    fn parse_url_only() {
        let config = ChannelConfig::default();

        let channel = Channel::from_str("http://localhost:1234", &config).unwrap();
        assert_eq!(
            channel.base_url,
            Url::from_str("http://localhost:1234/").unwrap()
        );
        assert_eq!(channel.name, None);
        assert_eq!(channel.platforms, None);

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
        let config = ChannelConfig::default();

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
        assert_eq!(channel.platforms, Some(smallvec![platform]));

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
        assert_eq!(channel.platforms, Some(smallvec![platform]));
    }
}
