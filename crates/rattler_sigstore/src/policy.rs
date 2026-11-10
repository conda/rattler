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

    /// The GitHub Actions OIDC issuer of a GitHub Enterprise Cloud tenant with
    /// data residency, `https://token.actions.<tenant>.ghe.com`, where
    /// `tenant` is the subdomain of the tenant's `<tenant>.ghe.com` host.
    pub fn github_actions_tenant(tenant: &str) -> Self {
        Self::new(format!("https://token.actions.{tenant}.ghe.com"))
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

/// A GitHub artifact attestation trust domain: the Sigstore instance GitHub
/// signs attestations of private and internal repositories with.
///
/// github.com uses one instance ([`Self::github_com`]). GitHub Enterprise Cloud
/// tenants with data residency (`<tenant>.ghe.com`) use the instance of their
/// region ([`Self::region`]), such as `prod-weu-01`, which the tenant's
/// `GET /meta` REST endpoint reports as
/// `domains.artifact_attestations.trust_domain`. GitHub publishes the trusted
/// root of every trust domain in its TUF repository (`tuf-repo.github.com`),
/// which is where [`crate::github_trusted_root`] fetches it from.
///
/// Parses from `github.com` or a region name, and displays the same way.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitHubTrustDomain {
    /// `None` for github.com.
    region: Option<String>,
}

/// A GitHub trust domain name that is not `github.com` and not a valid region
/// name (ASCII letters, digits and `-`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid GitHub trust domain {0:?}: expected github.com or a region such as prod-weu-01")]
pub struct InvalidGitHubTrustDomain(pub String);

impl GitHubTrustDomain {
    /// The trust domain of github.com.
    pub fn github_com() -> Self {
        Self { region: None }
    }

    /// The trust domain of a GitHub Enterprise Cloud data residency region,
    /// such as `prod-weu-01`.
    pub fn region(name: impl Into<String>) -> Result<Self, InvalidGitHubTrustDomain> {
        let name = name.into();
        let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if valid {
            Ok(Self { region: Some(name) })
        } else {
            Err(InvalidGitHubTrustDomain(name))
        }
    }

    /// The region name, or `None` for github.com.
    pub fn region_name(&self) -> Option<&str> {
        self.region.as_deref()
    }

    /// Whether this is github.com's trust domain.
    pub fn is_github_com(&self) -> bool {
        self.region.is_none()
    }

    /// The TUF target GitHub publishes this trust domain's trusted root as.
    pub(crate) fn trusted_root_target(&self) -> String {
        match &self.region {
            None => "trusted_root.json".to_string(),
            Some(region) => format!("{region}.trusted_root.json"),
        }
    }

    /// Whether this trust domain's instance issues certificates for `issuer`:
    /// github.com's for github.com workflows, a region's for the workflows of
    /// GitHub Enterprise Cloud tenants. Which region a tenant lives in is not
    /// part of its issuer, so any region accepts any tenant.
    pub fn issues_for(&self, issuer: &GitHubActionsIssuer) -> bool {
        match issuer {
            GitHubActionsIssuer::GitHubCom => self.is_github_com(),
            GitHubActionsIssuer::Tenant(_) => !self.is_github_com(),
        }
    }
}

impl std::str::FromStr for GitHubTrustDomain {
    type Err = InvalidGitHubTrustDomain;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("github.com") {
            Ok(Self::github_com())
        } else {
            Self::region(s)
        }
    }
}

impl std::fmt::Display for GitHubTrustDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.region.as_deref().unwrap_or("github.com"))
    }
}

/// A GitHub Actions OIDC issuer, which identifies the GitHub a workflow ran on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GitHubActionsIssuer {
    /// `https://token.actions.githubusercontent.com`: github.com.
    GitHubCom,
    /// `https://token.actions.<tenant>.ghe.com`: a GitHub Enterprise Cloud
    /// tenant with data residency.
    Tenant(String),
}

impl GitHubActionsIssuer {
    /// Recognizes a GitHub Actions issuer, including the `/<enterprise>` path
    /// of enterprises with a customized issuer. Returns `None` for any other
    /// issuer.
    pub fn parse(issuer: &str) -> Option<Self> {
        let url = Url::parse(issuer).ok()?;
        let plain = url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none();
        if !plain {
            return None;
        }
        let host = url.host_str()?;
        if host == "token.actions.githubusercontent.com" {
            return Some(Self::GitHubCom);
        }
        let tenant = host
            .strip_prefix("token.actions.")?
            .strip_suffix(".ghe.com")?;
        let valid = !tenant.is_empty()
            && tenant
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-');
        valid.then(|| Self::Tenant(tenant.to_string()))
    }
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

    /// The GitHub trust domains whose trusted roots [`crate::verify_record`]
    /// loads when the public-good root does not accept a package.
    github_trust_domains: Vec<GitHubTrustDomain>,
}

impl VerificationConfig {
    /// Creates a configuration that applies `publisher` to every package.
    ///
    /// Use `Publisher::new()` explicitly to accept any cryptographically valid
    /// signer. Different publishers per channel are not supported.
    ///
    /// Like `gh attestation verify`, this accepts attestations signed with the
    /// Sigstore public-good instance and with GitHub's own instance for
    /// github.com, which signs the attestations of private and internal
    /// repositories. Change that with [`Self::with_github_trust_domains`].
    pub fn new(publisher: Publisher) -> Self {
        Self {
            publisher,
            max_sidecar_size: DEFAULT_MAX_SIDECAR_SIZE,
            channel_check: ChannelCheck::default(),
            github_trust_domains: vec![GitHubTrustDomain::github_com()],
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

    /// Sets the GitHub trust domains whose attestations
    /// [`crate::verify_record`] accepts in addition to those signed with the
    /// Sigstore public-good instance. The default is
    /// [`GitHubTrustDomain::github_com`], like `gh attestation verify`; pass
    /// an empty list to only accept attestations that are recorded in the
    /// public transparency log of the public-good instance.
    ///
    /// GitHub signs attestations of private and internal repositories with its
    /// own Sigstore instance, which keeps no public transparency log so that
    /// nothing about the repository is published. Such a bundle carries an
    /// RFC 3161 timestamp from GitHub's timestamp authority instead of a log
    /// entry, so trusting it means trusting GitHub's certificate and timestamp
    /// authorities without public auditability. That holds for every
    /// repository a [`Publisher`] matches, public ones included: a workflow
    /// can sign with GitHub's instance whatever the visibility of its
    /// repository. Only certificates issued for
    /// GitHub Actions identities are accepted from these instances; the
    /// [`Publisher`] constraints apply as usual.
    ///
    /// Add the trust domain of a GitHub Enterprise Cloud data residency
    /// region, such as `prod-weu-01`, to accept attestations from its
    /// `*.ghe.com` tenants; regions are never trusted by default. Their identities are issued by
    /// [`Issuer::github_actions_tenant`].
    ///
    /// This only selects which roots [`crate::verify_record`] fetches. The
    /// functions taking [`crate::TrustedRoots`] trust exactly the roots they
    /// are given.
    pub fn with_github_trust_domains(
        mut self,
        trust_domains: impl IntoIterator<Item = GitHubTrustDomain>,
    ) -> Self {
        self.github_trust_domains = trust_domains.into_iter().collect();
        self
    }

    /// The GitHub trust domains whose attestations are accepted. See
    /// [`Self::with_github_trust_domains`].
    pub fn github_trust_domains(&self) -> &[GitHubTrustDomain] {
        &self.github_trust_domains
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
    fn github_actions_issuers() {
        for (issuer, expected) in [
            (
                "https://token.actions.githubusercontent.com",
                GitHubActionsIssuer::GitHubCom,
            ),
            (
                "https://token.actions.githubusercontent.com/octocorp",
                GitHubActionsIssuer::GitHubCom,
            ),
            (
                "https://token.actions.octocorp.ghe.com",
                GitHubActionsIssuer::Tenant("octocorp".to_string()),
            ),
            (
                "https://token.actions.octo-corp.ghe.com/octocorp",
                GitHubActionsIssuer::Tenant("octo-corp".to_string()),
            ),
        ] {
            assert_eq!(
                GitHubActionsIssuer::parse(issuer),
                Some(expected),
                "{issuer}"
            );
        }
        for issuer in [
            "https://token.actions.githubusercontent.com.evil.example",
            "https://token.actions.ghe.com",
            "https://token.actions.evil.example.ghe.com.evil.example",
            "https://token.actions.a.b.ghe.com",
            "http://token.actions.githubusercontent.com",
            "https://token.actions.githubusercontent.com:8443",
            "https://user@token.actions.githubusercontent.com",
            "https://gitlab.com",
            "not a url",
        ] {
            assert_eq!(GitHubActionsIssuer::parse(issuer), None, "{issuer}");
        }
    }

    #[test]
    fn github_trust_domains() {
        let github_com: GitHubTrustDomain = "github.com".parse().unwrap();
        assert!(github_com.is_github_com());
        assert_eq!(github_com.to_string(), "github.com");
        assert_eq!(github_com.trusted_root_target(), "trusted_root.json");

        let region: GitHubTrustDomain = "prod-weu-01".parse().unwrap();
        assert_eq!(region.region_name(), Some("prod-weu-01"));
        assert_eq!(region.to_string(), "prod-weu-01");
        assert_eq!(
            region.trusted_root_target(),
            "prod-weu-01.trusted_root.json"
        );

        for invalid in ["", "../root", "prod/weu", "prod weu", "prod.weu"] {
            assert!(GitHubTrustDomain::region(invalid).is_err(), "{invalid:?}");
        }

        // Each instance only issues for the identities of its own domain.
        let tenant = GitHubActionsIssuer::Tenant("octocorp".to_string());
        assert!(github_com.issues_for(&GitHubActionsIssuer::GitHubCom));
        assert!(!github_com.issues_for(&tenant));
        assert!(region.issues_for(&tenant));
        assert!(!region.issues_for(&GitHubActionsIssuer::GitHubCom));
    }

    #[test]
    fn only_github_com_is_trusted_by_default() {
        let config = VerificationConfig::new(Publisher::new());
        assert_eq!(
            config.github_trust_domains(),
            [GitHubTrustDomain::github_com()]
        );

        // The list is replaced, not extended: regions have to be named, and
        // an empty list leaves only the public-good instance.
        let region = GitHubTrustDomain::region("prod-weu-01").unwrap();
        let config = config.with_github_trust_domains([region.clone()]);
        assert_eq!(config.github_trust_domains(), [region]);
        let config = config.with_github_trust_domains([]);
        assert!(config.github_trust_domains().is_empty());
    }

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
