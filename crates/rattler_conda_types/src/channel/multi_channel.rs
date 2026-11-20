//! Multichannels: named, ordered groups of channels that are requested as a
//! whole, like the `defaults` channel of conda or its
//! `custom_multichannels`.

use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

use indexmap::IndexMap;
use thiserror::Error;

use super::{Channel, ChannelConfig, NamedChannelOrUrl, ParseChannelError};

/// The definitions of multichannels, by name. Each entry lists the channels
/// of the multichannel in order of preference.
///
/// This is the shape of the `multichannels` table of the configuration:
///
/// ```toml
/// [multichannels]
/// defaults = ["https://repo.anaconda.com/pkgs/main", "https://repo.anaconda.com/pkgs/r"]
/// ```
pub type MultiChannels = IndexMap<String, Vec<NamedChannelOrUrl>>;

/// A named, ordered group of channels that is requested as a whole.
///
/// When solving, the channels of a multichannel share a single channel
/// priority tier and a match spec whose channel is the name of the
/// multichannel (e.g. `defaults::python`) accepts records from every one of
/// them. Records keep referring to the channel they came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MultiChannel {
    /// The name of the multichannel.
    pub name: String,

    /// The channels of the multichannel, in order of preference.
    pub channels: Vec<Channel>,
}

/// A resolved channel: either a single channel or a multichannel.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelOrMultiChannel {
    /// A single channel.
    Channel(Channel),

    /// A multichannel.
    MultiChannel(MultiChannel),
}

impl ChannelOrMultiChannel {
    /// Returns the channels this resolves to, in order of preference.
    pub fn channels(&self) -> &[Channel] {
        match self {
            ChannelOrMultiChannel::Channel(channel) => std::slice::from_ref(channel),
            ChannelOrMultiChannel::MultiChannel(multi_channel) => &multi_channel.channels,
        }
    }

    /// Returns the name of the multichannel, or `None` for a single channel.
    pub fn multi_channel_name(&self) -> Option<&str> {
        match self {
            ChannelOrMultiChannel::Channel(_) => None,
            ChannelOrMultiChannel::MultiChannel(multi_channel) => Some(&multi_channel.name),
        }
    }
}

impl From<Channel> for ChannelOrMultiChannel {
    fn from(channel: Channel) -> Self {
        ChannelOrMultiChannel::Channel(channel)
    }
}

impl From<MultiChannel> for ChannelOrMultiChannel {
    fn from(multi_channel: MultiChannel) -> Self {
        ChannelOrMultiChannel::MultiChannel(multi_channel)
    }
}

/// Returns true if `name` can be used as the name of a multichannel.
///
/// A name must be non-empty and must not contain a path separator, a `:`
/// (which every URL and Windows path contains), a `,`, a `=` or whitespace.
/// This keeps a multichannel name distinguishable from a channel URL or path
/// and makes it usable in the `NAME=CHANNEL,CHANNEL,...` syntax of
/// [`ChannelSpecifier`].
pub fn is_valid_multi_channel_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', ':', ',', '='])
        && !name.chars().any(char::is_whitespace)
}

/// An error that can occur when defining or resolving a multichannel.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MultiChannelError {
    /// The name is not a valid multichannel name, see
    /// [`is_valid_multi_channel_name`].
    #[error(
        "'{0}' is not a valid multichannel name: it must be non-empty and must not contain '/', '\\', ':', ',', '=' or whitespace"
    )]
    InvalidName(String),

    /// The multichannel does not contain any channels.
    #[error("multichannel '{0}' does not contain any channels")]
    Empty(String),

    /// The multichannel contains another multichannel.
    #[error("multichannel '{name}' cannot contain another multichannel ('{nested}')")]
    Nested {
        /// The name of the multichannel.
        name: String,
        /// The name of the multichannel it contains.
        nested: String,
    },

    /// The multichannel contains the same channel more than once.
    #[error("multichannel '{name}' contains '{channel}' more than once")]
    DuplicateChannel {
        /// The name of the multichannel.
        name: String,
        /// The channel that occurs more than once.
        channel: String,
    },

    /// A channel could not be parsed.
    #[error(transparent)]
    InvalidChannel(#[from] ParseChannelError),
}

/// Checks that `name` and `channels` form a valid multichannel definition
/// given the other `multi_channels`: the name is valid, there is at least one
/// channel, no channel is named after a multichannel, and no channel occurs
/// more than once.
///
/// Duplicates are detected by how the channels are written, the URLs they
/// resolve to are compared when the multichannel is resolved.
pub fn validate_multi_channel(
    name: &str,
    channels: &[NamedChannelOrUrl],
    multi_channels: &MultiChannels,
) -> Result<(), MultiChannelError> {
    if !is_valid_multi_channel_name(name) {
        return Err(MultiChannelError::InvalidName(name.to_string()));
    }
    if channels.is_empty() {
        return Err(MultiChannelError::Empty(name.to_string()));
    }
    let mut seen = std::collections::HashSet::new();
    for channel in channels {
        if let NamedChannelOrUrl::Name(member) = channel
            && (member == name || multi_channels.contains_key(member))
        {
            return Err(MultiChannelError::Nested {
                name: name.to_string(),
                nested: member.clone(),
            });
        }
        if !seen.insert(channel.as_str()) {
            return Err(MultiChannelError::DuplicateChannel {
                name: name.to_string(),
                channel: channel.to_string(),
            });
        }
    }
    Ok(())
}

/// Resolves the channels of a multichannel called `name` and checks that it
/// is a valid multichannel given the other `multi_channels`.
fn resolve_multi_channel(
    name: &str,
    channels: Vec<NamedChannelOrUrl>,
    multi_channels: &MultiChannels,
    config: &ChannelConfig,
) -> Result<MultiChannel, MultiChannelError> {
    validate_multi_channel(name, &channels, multi_channels)?;
    let channels = channels
        .into_iter()
        .map(|channel| channel.into_channel(config))
        .collect::<Result<Vec<_>, _>>()?;

    let mut seen = std::collections::HashSet::new();
    for channel in &channels {
        if !seen.insert(&channel.base_url) {
            return Err(MultiChannelError::DuplicateChannel {
                name: name.to_string(),
                channel: channel.base_url.to_string(),
            });
        }
    }

    Ok(MultiChannel {
        name: name.to_string(),
        channels,
    })
}

impl NamedChannelOrUrl {
    /// Resolves this channel against the given multichannel definitions: a
    /// name that refers to one of `multi_channels` resolves to that
    /// multichannel, anything else to a single channel (see
    /// [`NamedChannelOrUrl::into_channel`]).
    pub fn into_channel_or_multi_channel(
        self,
        multi_channels: &MultiChannels,
        config: &ChannelConfig,
    ) -> Result<ChannelOrMultiChannel, MultiChannelError> {
        if let NamedChannelOrUrl::Name(name) = &self
            && let Some(channels) = multi_channels.get(name)
        {
            return resolve_multi_channel(name, channels.clone(), multi_channels, config)
                .map(ChannelOrMultiChannel::MultiChannel);
        }
        Ok(ChannelOrMultiChannel::Channel(self.into_channel(config)?))
    }
}

/// A channel as it can be specified on the command line: a single channel,
/// or an inline multichannel written as `NAME=CHANNEL,CHANNEL,...`.
///
/// Only a valid multichannel name (see [`is_valid_multi_channel_name`])
/// before the `=` makes a multichannel. URLs and Windows paths contain a `:`
/// and other paths a separator, so a channel that happens to contain a `=`
/// stays a channel.
///
/// ```
/// # use rattler_conda_types::ChannelSpecifier;
/// let spec: ChannelSpecifier = "defaults=https://repo.anaconda.com/pkgs/main,https://repo.anaconda.com/pkgs/r"
///     .parse()
///     .unwrap();
/// assert!(matches!(spec, ChannelSpecifier::MultiChannel { .. }));
///
/// let spec: ChannelSpecifier = "conda-forge".parse().unwrap();
/// assert!(matches!(spec, ChannelSpecifier::Channel(_)));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelSpecifier {
    /// A channel name, URL or path. A name may refer to a multichannel that
    /// is defined elsewhere, e.g. in the configuration.
    Channel(NamedChannelOrUrl),

    /// An inline multichannel called `name` that consists of `channels`.
    MultiChannel {
        /// The name of the multichannel.
        name: String,
        /// The channels of the multichannel, in order of preference.
        channels: Vec<NamedChannelOrUrl>,
    },
}

impl ChannelSpecifier {
    /// Resolves this specifier against the given multichannel definitions.
    ///
    /// A [`ChannelSpecifier::Channel`] is resolved with
    /// [`NamedChannelOrUrl::into_channel_or_multi_channel`]. The channels of
    /// an inline [`ChannelSpecifier::MultiChannel`] must not refer to
    /// another multichannel. An inline multichannel takes precedence over a
    /// definition in `multi_channels` with the same name.
    pub fn resolve(
        self,
        multi_channels: &MultiChannels,
        config: &ChannelConfig,
    ) -> Result<ChannelOrMultiChannel, MultiChannelError> {
        match self {
            ChannelSpecifier::Channel(channel) => {
                channel.into_channel_or_multi_channel(multi_channels, config)
            }
            ChannelSpecifier::MultiChannel { name, channels } => {
                resolve_multi_channel(&name, channels, multi_channels, config)
                    .map(ChannelOrMultiChannel::MultiChannel)
            }
        }
    }
}

/// An error that can occur when parsing a [`ChannelSpecifier`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ParseChannelSpecifierError {
    /// A channel of the multichannel is empty, e.g. `defaults=main,,r`.
    #[error(
        "expected NAME=CHANNEL,CHANNEL,... but '{0}' contains an empty channel (for example, defaults=pkgs/main,pkgs/r)"
    )]
    EmptyChannel(String),

    /// A channel could not be parsed.
    #[error("invalid channel '{channel}'")]
    InvalidChannel {
        /// The channel that could not be parsed.
        channel: String,
        /// The reason it could not be parsed.
        #[source]
        source: url::ParseError,
    },
}

impl FromStr for ChannelSpecifier {
    type Err = ParseChannelSpecifierError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parse = |channel: &str| {
            NamedChannelOrUrl::from_str(channel).map_err(|source| {
                ParseChannelSpecifierError::InvalidChannel {
                    channel: channel.to_string(),
                    source,
                }
            })
        };

        let Some((name, channels)) = s
            .split_once('=')
            .filter(|(name, _)| is_valid_multi_channel_name(name))
        else {
            return parse(s).map(ChannelSpecifier::Channel);
        };
        let channels = channels
            .split(',')
            .map(|channel| {
                if channel.is_empty() {
                    Err(ParseChannelSpecifierError::EmptyChannel(s.to_string()))
                } else {
                    parse(channel)
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(ChannelSpecifier::MultiChannel {
            name: name.to_string(),
            channels,
        })
    }
}

impl Display for ChannelSpecifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelSpecifier::Channel(channel) => write!(f, "{channel}"),
            ChannelSpecifier::MultiChannel { name, channels } => {
                write!(f, "{name}=")?;
                for (index, channel) in channels.iter().enumerate() {
                    if index > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{channel}")?;
                }
                Ok(())
            }
        }
    }
}

impl From<NamedChannelOrUrl> for ChannelSpecifier {
    fn from(channel: NamedChannelOrUrl) -> Self {
        ChannelSpecifier::Channel(channel)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn config() -> ChannelConfig {
        ChannelConfig::default_with_root_dir(PathBuf::from("/"))
    }

    fn defaults() -> MultiChannels {
        let mut multi_channels = MultiChannels::new();
        multi_channels.insert(
            "defaults".to_string(),
            vec![
                "https://repo.anaconda.com/pkgs/main".parse().unwrap(),
                "https://repo.anaconda.com/pkgs/r".parse().unwrap(),
            ],
        );
        multi_channels
    }

    #[test]
    fn parse_channel_specifier() {
        assert_eq!(
            "conda-forge".parse::<ChannelSpecifier>().unwrap(),
            ChannelSpecifier::Channel(NamedChannelOrUrl::Name("conda-forge".to_string()))
        );
        assert_eq!(
            "defaults=pkgs/main,https://repo.anaconda.com/pkgs/r"
                .parse::<ChannelSpecifier>()
                .unwrap(),
            ChannelSpecifier::MultiChannel {
                name: "defaults".to_string(),
                channels: vec![
                    "pkgs/main".parse().unwrap(),
                    "https://repo.anaconda.com/pkgs/r".parse().unwrap(),
                ],
            }
        );

        // A URL or path with a `=` stays a channel.
        for channel in [
            "https://example.com/channel?token=abc",
            "./local=channel",
            "C:\\channels\\a=b",
        ] {
            assert!(
                matches!(
                    channel.parse::<ChannelSpecifier>().unwrap(),
                    ChannelSpecifier::Channel(_)
                ),
                "{channel}"
            );
        }

        assert_eq!(
            "defaults=main,,r".parse::<ChannelSpecifier>(),
            Err(ParseChannelSpecifierError::EmptyChannel(
                "defaults=main,,r".to_string()
            ))
        );
        assert!("defaults=".parse::<ChannelSpecifier>().is_err());
    }

    #[test]
    fn channel_specifier_display_roundtrip() {
        for s in ["conda-forge", "defaults=pkgs/main,https://example.com/r"] {
            assert_eq!(s.parse::<ChannelSpecifier>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn resolve_named_multi_channel() {
        let resolved = NamedChannelOrUrl::Name("defaults".to_string())
            .into_channel_or_multi_channel(&defaults(), &config())
            .unwrap();
        assert_eq!(resolved.multi_channel_name(), Some("defaults"));
        let urls: Vec<_> = resolved
            .channels()
            .iter()
            .map(|channel| channel.base_url.as_str().to_string())
            .collect();
        assert_eq!(
            urls,
            [
                "https://repo.anaconda.com/pkgs/main/",
                "https://repo.anaconda.com/pkgs/r/"
            ]
        );

        let resolved = NamedChannelOrUrl::Name("conda-forge".to_string())
            .into_channel_or_multi_channel(&defaults(), &config())
            .unwrap();
        assert_eq!(resolved.multi_channel_name(), None);
        assert_eq!(resolved.channels()[0].name(), "conda-forge");
    }

    #[test]
    fn resolve_inline_multi_channel_overrides_definition() {
        let resolved: ChannelOrMultiChannel = "defaults=conda-forge"
            .parse::<ChannelSpecifier>()
            .unwrap()
            .resolve(&defaults(), &config())
            .unwrap();
        assert_eq!(resolved.channels().len(), 1);
        assert_eq!(resolved.channels()[0].name(), "conda-forge");

        // A channel name with a `/` resolves against the channel alias.
        let resolved = "defaults=pkgs/main"
            .parse::<ChannelSpecifier>()
            .unwrap()
            .resolve(&MultiChannels::new(), &config())
            .unwrap();
        assert_eq!(
            resolved.channels()[0].base_url.as_str(),
            "https://conda.anaconda.org/pkgs/main/"
        );
    }

    #[test]
    fn invalid_multi_channels() {
        let multi_channels = defaults();
        let resolve = |s: &str| {
            s.parse::<ChannelSpecifier>()
                .unwrap()
                .resolve(&multi_channels, &config())
        };

        assert_eq!(
            resolve("corp=conda-forge,defaults"),
            Err(MultiChannelError::Nested {
                name: "corp".to_string(),
                nested: "defaults".to_string()
            })
        );
        assert!(matches!(
            resolve("corp=conda-forge,conda-forge"),
            Err(MultiChannelError::DuplicateChannel { .. })
        ));
        // The same channel written differently is a duplicate too.
        assert!(matches!(
            resolve("corp=conda-forge,https://conda.anaconda.org/conda-forge"),
            Err(MultiChannelError::DuplicateChannel { .. })
        ));

        assert_eq!(
            validate_multi_channel("a/b", &[], &MultiChannels::new()),
            Err(MultiChannelError::InvalidName("a/b".to_string()))
        );
        assert_eq!(
            validate_multi_channel("empty", &[], &MultiChannels::new()),
            Err(MultiChannelError::Empty("empty".to_string()))
        );
    }
}
