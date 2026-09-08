//! Trust policy: which signing identities are accepted for which channels and
//! how strictly verification failures are treated.

use std::fmt;

use url::Url;

use crate::sidecar::DEFAULT_MAX_SIDECAR_SIZE;

/// The identity (Subject Alternative Name) of a signing certificate, e.g. a
/// GitHub Actions workflow identity such as
/// `https://github.com/org/repo/.github/workflows/build.yml@refs/heads/main`.
///
/// An identity may contain `*` wildcards that match any sequence of
/// characters, so `https://github.com/org/repo/*` accepts every workflow and
/// ref of a repository.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identity(String);

impl Identity {
    /// Creates a new identity pattern.
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }

    /// Returns the pattern as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns true if `actual` matches this pattern.
    pub fn matches(&self, actual: &str) -> bool {
        wildcard_match(&self.0, actual)
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Identity {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Identity {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// The OIDC issuer that vouched for a signing identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Issuer(String);

impl Issuer {
    /// Creates a new issuer.
    pub fn new(issuer: impl Into<String>) -> Self {
        Self(issuer.into())
    }

    /// The GitHub Actions OIDC issuer.
    pub fn github_actions() -> Self {
        Self::new("https://token.actions.githubusercontent.com")
    }

    /// The GitLab CI OIDC issuer.
    pub fn gitlab() -> Self {
        Self::new("https://gitlab.com")
    }

    /// Returns the issuer as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Issuer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Issuer {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Issuer {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// A trusted publisher: constraints on the identity and issuer of a signing
/// certificate. A constraint that is `None` accepts any value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Publisher {
    /// The identity pattern the certificate must match, if any.
    pub identity: Option<Identity>,
    /// The issuer the certificate must have been issued for, if any.
    pub issuer: Option<Issuer>,
}

impl Publisher {
    /// Creates a publisher without constraints, which accepts any valid
    /// signature.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requires the certificate identity to match `identity`.
    pub fn with_identity(mut self, identity: impl Into<Identity>) -> Self {
        self.identity = Some(identity.into());
        self
    }

    /// Requires the certificate to be issued for `issuer`.
    pub fn with_issuer(mut self, issuer: impl Into<Issuer>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Returns true if the certificate identity and issuer satisfy this
    /// publisher's constraints. A constrained field that is absent from the
    /// certificate never matches.
    pub fn matches(&self, cert_identity: Option<&str>, cert_issuer: Option<&str>) -> bool {
        let identity_ok = match &self.identity {
            None => true,
            Some(pattern) => cert_identity.is_some_and(|actual| pattern.matches(actual)),
        };
        let issuer_ok = match &self.issuer {
            None => true,
            Some(issuer) => cert_issuer == Some(issuer.as_str()),
        };
        identity_ok && issuer_ok
    }
}

/// How the `targetChannel` recorded in an attestation is compared against the
/// channel a package was retrieved from.
///
/// CEP 27 says the field SHOULD match, but allows verifiers to permit
/// mismatches for mirrors.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChannelCheck {
    /// A mismatching `targetChannel` rejects the attestation.
    #[default]
    Require,
    /// A mismatching `targetChannel` is reported as a warning.
    Warn,
    /// `targetChannel` is not compared.
    Ignore,
}

/// Configuration shared by the `Warn` and `Require` verification modes.
#[derive(Debug, Clone)]
pub struct VerificationConfig {
    /// Publishers accepted for packages whose URL starts with the given
    /// channel URL. The most specific (longest) matching prefix wins.
    channel_publishers: Vec<(Url, Vec<Publisher>)>,

    /// Publishers accepted for packages from channels that have no explicit
    /// entry. `None` means such packages cannot be verified.
    default_publishers: Option<Vec<Publisher>>,

    /// The maximum sidecar size in bytes.
    max_sidecar_size: u64,

    /// How `targetChannel` is checked.
    channel_check: ChannelCheck,
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            channel_publishers: Vec::new(),
            default_publishers: None,
            max_sidecar_size: DEFAULT_MAX_SIDECAR_SIZE,
            channel_check: ChannelCheck::default(),
        }
    }
}

impl VerificationConfig {
    /// Creates an empty configuration: no channel is mapped and there are no
    /// default publishers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts `publisher` for packages served from `channel_url` (a URL
    /// prefix, compared on path segment boundaries).
    pub fn add_channel_publisher(&mut self, channel_url: Url, publisher: Publisher) -> &mut Self {
        let channel_url = normalize_channel_url(channel_url);
        match self
            .channel_publishers
            .iter_mut()
            .find(|(url, _)| *url == channel_url)
        {
            Some((_, publishers)) => publishers.push(publisher),
            None => self.channel_publishers.push((channel_url, vec![publisher])),
        }
        self
    }

    /// Builder variant of [`Self::add_channel_publisher`].
    pub fn with_channel_publisher(mut self, channel_url: Url, publisher: Publisher) -> Self {
        self.add_channel_publisher(channel_url, publisher);
        self
    }

    /// Accepts `publishers` for packages from channels without an explicit
    /// entry. An empty list accepts any valid signature from such channels.
    pub fn with_default_publishers(mut self, publishers: Vec<Publisher>) -> Self {
        self.default_publishers = Some(publishers);
        self
    }

    /// Sets the maximum sidecar size in bytes.
    pub fn with_max_sidecar_size(mut self, max_size: u64) -> Self {
        self.max_sidecar_size = max_size;
        self
    }

    /// Sets how `targetChannel` is checked.
    pub fn with_channel_check(mut self, channel_check: ChannelCheck) -> Self {
        self.channel_check = channel_check;
        self
    }

    /// The maximum sidecar size in bytes.
    pub fn max_sidecar_size(&self) -> u64 {
        self.max_sidecar_size
    }

    /// How `targetChannel` is checked.
    pub fn channel_check(&self) -> ChannelCheck {
        self.channel_check
    }

    /// Returns the publishers accepted for a package at `package_url`, or
    /// `None` if the package's channel is not mapped and there are no default
    /// publishers.
    pub fn publishers_for_url(&self, package_url: &Url) -> Option<&[Publisher]> {
        self.channel_publishers
            .iter()
            .filter_map(|(channel, publishers)| {
                url_prefix_specificity(channel, package_url).map(|len| (len, publishers))
            })
            .max_by_key(|(len, _)| *len)
            .map(|(_, publishers)| publishers.as_slice())
            .or(self.default_publishers.as_deref())
    }
}

/// Controls whether and how strictly attestations are verified.
#[derive(Debug, Clone, Default)]
pub enum VerificationPolicy {
    /// Attestations are not looked at.
    #[default]
    Disabled,
    /// Attestations are verified; problems are reported as warnings and never
    /// block an installation.
    Warn(VerificationConfig),
    /// Every package must have an attestation that verifies against a
    /// configured publisher. Anything else is an error.
    Require(VerificationConfig),
}

impl VerificationPolicy {
    /// Returns true unless the policy is [`VerificationPolicy::Disabled`].
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// Returns true if failures must block installation.
    pub fn is_required(&self) -> bool {
        matches!(self, Self::Require(_))
    }

    /// The configuration, if verification is enabled.
    pub fn config(&self) -> Option<&VerificationConfig> {
        match self {
            Self::Disabled => None,
            Self::Warn(config) | Self::Require(config) => Some(config),
        }
    }
}

/// Strips a trailing slash from the channel URL path so that prefixes compare
/// consistently.
pub(crate) fn normalize_channel_url(mut url: Url) -> Url {
    let trimmed = url.path().trim_end_matches('/').to_owned();
    url.set_path(&trimmed);
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// Returns the number of path segments of `prefix` if `package_url` lives under
/// it, taking segment boundaries into account so that `/conda-forge` does not
/// match `/conda-forge-extras/...`.
fn url_prefix_specificity(prefix: &Url, package_url: &Url) -> Option<usize> {
    if prefix.scheme() != package_url.scheme()
        || prefix.host_str() != package_url.host_str()
        || prefix.port_or_known_default() != package_url.port_or_known_default()
    {
        return None;
    }
    let prefix_segments: Vec<&str> = prefix
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).collect())
        .unwrap_or_default();
    let package_segments: Vec<&str> = package_url
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).collect())
        .unwrap_or_default();
    package_segments
        .starts_with(&prefix_segments)
        .then_some(prefix_segments.len())
}

/// Matches `text` against `pattern`, where `*` matches any (possibly empty)
/// sequence of characters. All other characters match literally.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            backtrack = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = backtrack {
            p = star_p + 1;
            t = star_t + 1;
            backtrack = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matching() {
        assert!(wildcard_match("abc", "abc"));
        assert!(!wildcard_match("abc", "abd"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match(
            "https://github.com/org/repo/*",
            "https://github.com/org/repo/.github/workflows/build.yml@refs/heads/main"
        ));
        assert!(!wildcard_match(
            "https://github.com/org/repo/*",
            "https://github.com/org/other/.github/workflows/build.yml@refs/heads/main"
        ));
        assert!(wildcard_match("a*b*c", "aXXbYYc"));
        assert!(!wildcard_match("a*b*c", "aXXbYY"));
        assert!(wildcard_match(
            "https://github.com/org/*/.github/workflows/*.yml@refs/tags/*",
            "https://github.com/org/repo/.github/workflows/release.yml@refs/tags/v1.0"
        ));
    }

    #[test]
    fn publisher_matching() {
        let identity = "https://github.com/org/repo/.github/workflows/build.yml@refs/heads/main";
        let issuer = Issuer::github_actions();

        assert!(Publisher::new().matches(None, None));
        assert!(Publisher::new().matches(Some(identity), Some(issuer.as_str())));

        let publisher = Publisher::new()
            .with_identity("https://github.com/org/repo/*")
            .with_issuer(issuer.clone());
        assert!(publisher.matches(Some(identity), Some(issuer.as_str())));
        assert!(!publisher.matches(Some(identity), Some("https://gitlab.com")));
        assert!(!publisher.matches(
            Some("https://github.com/evil/repo/x"),
            Some(issuer.as_str())
        ));
        assert!(!publisher.matches(None, Some(issuer.as_str())));
        assert!(!publisher.matches(Some(identity), None));
    }

    #[test]
    fn publishers_longest_prefix_wins() {
        let a = Publisher::new().with_identity("a");
        let b = Publisher::new().with_identity("b");
        let d = Publisher::new().with_identity("d");
        let config = VerificationConfig::new()
            .with_channel_publisher(Url::parse("https://prefix.dev/").unwrap(), a.clone())
            .with_channel_publisher(
                Url::parse("https://prefix.dev/conda-forge/").unwrap(),
                b.clone(),
            )
            .with_default_publishers(vec![d.clone()]);

        let pkg = Url::parse("https://prefix.dev/conda-forge/linux-64/x-1-0.conda").unwrap();
        assert_eq!(config.publishers_for_url(&pkg), Some(&[b][..]));

        let pkg = Url::parse("https://prefix.dev/conda-forge-extras/linux-64/x-1-0.conda").unwrap();
        assert_eq!(config.publishers_for_url(&pkg), Some(&[a][..]));

        let pkg =
            Url::parse("https://conda.anaconda.org/conda-forge/linux-64/x-1-0.conda").unwrap();
        assert_eq!(config.publishers_for_url(&pkg), Some(&[d][..]));

        let config = VerificationConfig::new();
        assert_eq!(config.publishers_for_url(&pkg), None);
    }

    #[test]
    fn channel_publishers_accumulate() {
        let url = Url::parse("https://prefix.dev/channel").unwrap();
        let mut config = VerificationConfig::new();
        config.add_channel_publisher(url.clone(), Publisher::new().with_identity("a"));
        config.add_channel_publisher(
            Url::parse("https://prefix.dev/channel/").unwrap(),
            Publisher::new().with_identity("b"),
        );
        let pkg = Url::parse("https://prefix.dev/channel/noarch/x-1-0.conda").unwrap();
        assert_eq!(config.publishers_for_url(&pkg).unwrap().len(), 2);
    }
}
