//! Trust policy: which signing identity is accepted and how strictly
//! verification failures are treated.
//!
//! Neither [CEP 27](https://conda.org/learn/ceps/cep-0027) nor
//! [CEP 50](https://conda.org/learn/ceps/cep-0050) prescribes this — CEP 50
//! leaves trust policy explicitly out of scope — so everything here is
//! rattler's own policy layer on top of them.

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
/// [CEP 27](https://conda.org/learn/ceps/cep-0027) says the field SHOULD match,
/// but allows verifiers to permit mismatches for mirrors.
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
    /// The publisher accepted for every package, regardless of channel.
    publisher: Publisher,

    /// The maximum sidecar size in bytes.
    max_sidecar_size: u64,

    /// How `targetChannel` is checked.
    channel_check: ChannelCheck,
}

impl VerificationConfig {
    /// Creates a configuration that applies `publisher` to every package.
    ///
    /// Use `Publisher::new()` explicitly to accept any cryptographically valid
    /// signer. Different publishers per channel are not supported.
    pub fn new(publisher: Publisher) -> Self {
        Self {
            publisher,
            max_sidecar_size: DEFAULT_MAX_SIDECAR_SIZE,
            channel_check: ChannelCheck::default(),
        }
    }

    /// The publisher required for every package.
    pub fn publisher(&self) -> &Publisher {
        &self.publisher
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

/// Strips a trailing slash from the channel URL path for comparison with an
/// attestation's target channel.
pub(crate) fn normalize_channel_url(mut url: Url) -> Url {
    let trimmed = url.path().trim_end_matches('/').to_owned();
    url.set_path(&trimmed);
    url.set_query(None);
    url.set_fragment(None);
    url
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
        assert!(!wildcard_match("abc", "xyz"));
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
}
