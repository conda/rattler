//! OSC 8 terminal hyperlinks for the human-readable output.
//!
//! Hyperlinks are purely decorative here: the text of a link always stands on
//! its own, so a consumer that does not render OSC 8 (a pipe, a log file, an
//! agent reading the raw bytes) loses nothing. Links are only emitted when the
//! stream they are written to is an interactive terminal that is known to
//! support them, which also keeps the machine-readable output (`--format json`,
//! `--format urls`) and the snapshot tests free of escape codes.
//!
//! `FORCE_HYPERLINK=1` turns links on regardless (useful when piping into a
//! renderer), `FORCE_HYPERLINK=0` turns them off; `NO_COLOR` and `CLICOLOR=0`
//! disable them along with the rest of the styling.

use std::{fmt::Display, path::Path, str::FromStr, sync::LazyLock};

use rattler_conda_types::Platform;
pub use supports_hyperlinks::Stream;
use url::Url;

/// Channels that are mirrored on prefix.dev, which serves a nicer package page
/// than the channel host itself. Anything else keeps pointing at its origin.
const PREFIX_DEV_MIRRORS: &[&str] = &[
    "bioconda",
    "conda-forge",
    "main",
    "nvidia",
    "pytorch",
    "rapidsai",
    "robostack",
    "robostack-staging",
];

/// Whether hyperlinks should be written to `stream`.
///
/// Determined once per stream: the environment does not change during a run and
/// every call site would otherwise repeat the terminal probing. Each stream is
/// decided on its own, so redirecting only stdout keeps the links in the
/// progress messages on stderr.
fn enabled(stream: Stream) -> bool {
    fn detect(stream: Stream) -> bool {
        // An explicit request wins over the terminal detection, in both
        // directions.
        if let Ok(force) = std::env::var("FORCE_HYPERLINK") {
            return force.trim() != "0";
        }
        // Styling and hyperlinks are turned off by the same signals
        // (`NO_COLOR`, `CLICOLOR=0`, a redirected stream), so they are decided
        // together.
        let colors_enabled = match stream {
            Stream::Stdout => console::colors_enabled(),
            Stream::Stderr => console::colors_enabled_stderr(),
        };
        colors_enabled && supports_hyperlinks::on(stream)
    }

    static STDOUT: LazyLock<bool> = LazyLock::new(|| detect(Stream::Stdout));
    static STDERR: LazyLock<bool> = LazyLock::new(|| detect(Stream::Stderr));
    match stream {
        Stream::Stdout => *STDOUT,
        Stream::Stderr => *STDERR,
    }
}

/// Wraps `text` in an OSC 8 escape sequence pointing at `url`.
fn osc8(url: &Url, text: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// Renders `text` on stdout as a hyperlink to `url`, or unchanged when there is
/// no URL to link to or the terminal does not support hyperlinks.
pub fn maybe_link(url: Option<Url>, text: impl Display) -> String {
    maybe_link_on(Stream::Stdout, url, text)
}

/// [`maybe_link`] for text that is written to `stream` instead of to stdout.
pub fn maybe_link_on(stream: Stream, url: Option<Url>, text: impl Display) -> String {
    let text = text.to_string();
    match url {
        Some(url) if enabled(stream) => osc8(&url, &text),
        _ => text,
    }
}

/// `url` itself, if it is safe to turn into a clickable link.
///
/// Only `http(s)` URLs are, because these URLs come from package metadata: a
/// package should not be able to get a terminal to hand an arbitrary scheme to
/// the operating system.
pub fn web(url: &Url) -> Option<Url> {
    matches!(url.scheme(), "http" | "https").then(|| url.clone())
}

/// The `file://` URL of `path`, if it can be expressed as one.
///
/// The path must be absolute; a relative path has no meaningful `file://` URL.
pub fn file(path: &Path) -> Option<Url> {
    file_url::file_path_to_url(path.to_str()?).ok()
}

/// The `file://` URL of the directory `path`, if it can be expressed as one.
pub fn directory(path: &Path) -> Option<Url> {
    file_url::directory_path_to_url(path.to_str()?).ok()
}

/// The channel host, as far as it matters for building web page URLs.
#[derive(Debug, PartialEq, Eq)]
enum Host {
    /// A channel served by prefix.dev.
    PrefixDev,
    /// A channel served by Anaconda, which prefix.dev may mirror.
    Anaconda,
}

/// Splits a channel string into the host that serves it and its name.
///
/// The channel of a record is not necessarily a URL: it can also be a bare
/// name, which refers to a channel under the default channel alias.
fn parse_channel(channel: &str) -> Option<(Host, String)> {
    let Ok(url) = Url::parse(channel) else {
        // Only a plain name, no path and no scheme-like characters, can be
        // resolved against the default alias with any confidence.
        let is_plain_name = !channel.is_empty()
            && channel
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        return is_plain_name.then(|| (Host::Anaconda, channel.to_string()));
    };

    let mut segments: Vec<&str> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();

    // A channel is sometimes referred to by one of its subdirs; the web pages
    // are per channel, so the subdir is dropped.
    if let Some(last) = segments.last()
        && Platform::from_str(last).is_ok()
    {
        segments.pop();
    }

    match (url.host_str()?, segments.as_slice()) {
        ("prefix.dev" | "repo.prefix.dev", [name]) => Some((Host::PrefixDev, (*name).to_string())),
        ("anaconda.org" | "conda.anaconda.org", [name]) => {
            Some((Host::Anaconda, (*name).to_string()))
        }
        // The Anaconda distribution channels (`defaults`) live one level
        // deeper, e.g. `https://repo.anaconda.com/pkgs/main`.
        ("repo.anaconda.com", ["pkgs", name]) => Some((Host::Anaconda, (*name).to_string())),
        _ => None,
    }
}

/// The URL of the web page of `channel`, if it has a known one.
///
/// Channels that are not served by a host with a package index (a self-hosted
/// channel, a local directory) have no page and yield `None`.
pub fn channel_page(channel: &str) -> Option<Url> {
    let (host, name) = parse_channel(channel)?;
    let url = match host {
        Host::PrefixDev => format!("https://prefix.dev/channels/{name}"),
        Host::Anaconda if PREFIX_DEV_MIRRORS.contains(&name.as_str()) => {
            format!("https://prefix.dev/channels/{name}")
        }
        Host::Anaconda => format!("https://anaconda.org/{name}"),
    };
    Url::parse(&url).ok()
}

/// The URL of the web page of `package` in `channel`, if it has a known one.
pub fn package_page(channel: Option<&str>, package: &str) -> Option<Url> {
    let (host, name) = parse_channel(channel?)?;
    let url = match host {
        Host::PrefixDev => format!("https://prefix.dev/channels/{name}/packages/{package}"),
        Host::Anaconda if PREFIX_DEV_MIRRORS.contains(&name.as_str()) => {
            format!("https://prefix.dev/channels/{name}/packages/{package}")
        }
        Host::Anaconda => format!("https://anaconda.org/{name}/{package}"),
    };
    Url::parse(&url).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(channel: &str) -> Option<String> {
        package_page(Some(channel), "xtensor").map(Url::into)
    }

    #[test]
    fn test_osc8_wraps_the_text() {
        let url = Url::parse("https://prefix.dev/channels/conda-forge").unwrap();
        assert_eq!(
            osc8(&url, "conda-forge"),
            "\u{1b}]8;;https://prefix.dev/channels/conda-forge\u{1b}\\conda-forge\u{1b}]8;;\u{1b}\\"
        );
    }

    #[test]
    fn test_mirrored_channels_link_to_prefix_dev() {
        let expected = Some("https://prefix.dev/channels/conda-forge/packages/xtensor".to_string());
        // As a channel URL, as a bare name resolved against the default alias,
        // and as one of its subdirs.
        assert_eq!(page("https://conda.anaconda.org/conda-forge/"), expected);
        assert_eq!(page("conda-forge"), expected);
        assert_eq!(
            page("https://conda.anaconda.org/conda-forge/osx-arm64"),
            expected
        );
        assert_eq!(
            page("https://repo.anaconda.com/pkgs/main"),
            Some("https://prefix.dev/channels/main/packages/xtensor".to_string())
        );
    }

    #[test]
    fn test_other_anaconda_channels_link_to_anaconda_org() {
        assert_eq!(
            page("https://conda.anaconda.org/some-user/"),
            Some("https://anaconda.org/some-user/xtensor".to_string())
        );
    }

    #[test]
    fn test_prefix_dev_channels_link_to_their_own_page() {
        let expected = Some("https://prefix.dev/channels/my-channel/packages/xtensor".to_string());
        assert_eq!(page("https://repo.prefix.dev/my-channel"), expected);
        assert_eq!(page("https://prefix.dev/my-channel"), expected);
    }

    #[test]
    fn test_channels_without_a_known_page() {
        // Self-hosted channels, local directories and anything that is neither
        // a URL nor a plain channel name.
        assert_eq!(page("https://packages.example.com/conda/my-channel"), None);
        assert_eq!(page("file:///home/user/channel"), None);
        assert_eq!(page("/home/user/channel"), None);
        assert_eq!(page(""), None);
        // A host we know, but not a path we can interpret.
        assert_eq!(page("https://conda.anaconda.org/"), None);
        assert_eq!(page("https://repo.anaconda.com/pkgs/main/extra"), None);
    }

    #[test]
    fn test_channel_page() {
        assert_eq!(
            channel_page("https://conda.anaconda.org/conda-forge/").map(Url::into),
            Some("https://prefix.dev/channels/conda-forge".to_string())
        );
        assert_eq!(
            channel_page("https://conda.anaconda.org/some-user/").map(Url::into),
            Some("https://anaconda.org/some-user".to_string())
        );
        assert_eq!(channel_page("file:///home/user/channel"), None);
    }
}
