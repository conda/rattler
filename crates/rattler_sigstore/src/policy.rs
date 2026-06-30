//! Verification policy types and configuration.

use crate::error::{SigstoreError, SigstoreResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use url::Url;

/// A certificate identity (Subject Alternative Name).
///
/// This is the identity embedded in a Sigstore signing certificate,
/// typically a workflow identity URI for CI/CD systems.
///
/// # Examples
///
/// ```
/// use rattler_sigstore::Identity;
///
/// // GitHub Actions workflow identity
/// let identity = Identity::new("https://github.com/conda-forge/feedstock/.github/workflows/build.yaml@refs/heads/main");
///
/// // Wildcard pattern for all conda-forge workflows
/// let pattern = Identity::new("https://github.com/conda-forge/*");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Identity(String);

impl Identity {
    /// Create a new identity.
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }

    /// Get the identity as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Check if this identity matches the given actual identity.
    ///
    /// Supports wildcard matching with `*` suffix.
    pub fn matches(&self, actual: &str) -> bool {
        if let Some(prefix) = self.0.strip_suffix('*') {
            actual.starts_with(prefix)
        } else {
            self.0 == actual
        }
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Identity {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Identity {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl AsRef<str> for Identity {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for Identity {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A certificate issuer (OIDC provider).
///
/// This is the identity provider that issued the signing certificate,
/// such as GitHub Actions or GitLab CI.
///
/// # Common Issuers
///
/// - GitHub Actions: `https://token.actions.githubusercontent.com`
/// - GitLab CI: `https://gitlab.com`
/// - Google Cloud Build: `https://accounts.google.com`
///
/// # Examples
///
/// ```
/// use rattler_sigstore::Issuer;
///
/// let github = Issuer::github_actions();
/// let gitlab = Issuer::gitlab();
/// let custom = Issuer::new("https://custom-oidc.example.com");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Issuer(String);

impl Issuer {
    /// Create a new issuer.
    pub fn new(issuer: impl Into<String>) -> Self {
        Self(issuer.into())
    }

    /// GitHub Actions OIDC issuer.
    pub fn github_actions() -> Self {
        Self("https://token.actions.githubusercontent.com".to_string())
    }

    /// GitLab CI OIDC issuer.
    pub fn gitlab() -> Self {
        Self("https://gitlab.com".to_string())
    }

    /// Get the issuer as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Issuer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Issuer {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Issuer {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl AsRef<str> for Issuer {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for Issuer {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A publisher identity that can sign packages.
///
/// Publishers are identified by their certificate identity (SAN) and issuer.
/// At least one of `identity` or `issuer` should be specified.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Publisher {
    /// Required identity (Subject Alternative Name from the signing certificate).
    ///
    /// For GitHub Actions, this is typically a workflow identity URI like:
    /// `https://github.com/org/repo/.github/workflows/build.yaml@refs/heads/main`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,

    /// Required issuer of the signing certificate.
    ///
    /// For GitHub Actions, this is `https://token.actions.githubusercontent.com`.
    /// For GitLab CI, this is `https://gitlab.com`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<Issuer>,
}

impl Publisher {
    /// Create a new empty publisher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the required identity.
    pub fn with_identity(mut self, identity: impl Into<Identity>) -> Self {
        self.identity = Some(identity.into());
        self
    }

    /// Set the required issuer.
    pub fn with_issuer(mut self, issuer: impl Into<Issuer>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Check if this publisher has any constraints.
    pub fn has_constraints(&self) -> bool {
        self.identity.is_some() || self.issuer.is_some()
    }

    /// Check if a certificate identity and issuer match this publisher.
    pub fn matches(&self, cert_identity: Option<&str>, cert_issuer: Option<&str>) -> bool {
        // If we require an identity, check it matches
        if let Some(required_identity) = &self.identity {
            match cert_identity {
                Some(actual) if required_identity.matches(actual) => {}
                _ => return false,
            }
        }

        // If we require an issuer, check it matches
        if let Some(required_issuer) = &self.issuer {
            match cert_issuer {
                Some(actual) if required_issuer.as_str() == actual => {}
                _ => return false,
            }
        }

        true
    }
}

/// Configuration for signature verification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VerificationConfig {
    /// Mapping from channel URL prefix to allowed publishers.
    ///
    /// The longest matching prefix is used for a given package URL.
    /// For example, if you have entries for:
    /// - `https://conda.anaconda.org/conda-forge/`
    /// - `https://conda.anaconda.org/`
    ///
    /// A package from `https://conda.anaconda.org/conda-forge/linux-64/pkg.conda`
    /// will use the first (more specific) entry.
    #[serde(default)]
    pub channel_publishers: HashMap<String, Vec<Publisher>>,

    /// Default publishers to accept for channels not in `channel_publishers`.
    ///
    /// If `None`, packages from unmapped channels will fail verification in
    /// `Require` mode or be skipped (not verified) in `Warn` mode.
    ///
    /// If set to an empty `Vec` (or a [`Publisher`] with no constraints), any
    /// validly-signed package is accepted regardless of its signer identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_publishers: Option<Vec<Publisher>>,

    /// URL pattern for fetching signatures.
    ///
    /// Use `{url}` as a placeholder for the package URL.
    /// Default: `{url}.v0.sigs`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signatures_url_pattern: Option<String>,
}

impl VerificationConfig {
    /// Create a new empty configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a publisher for a specific channel.
    pub fn add_channel_publisher(&mut self, channel_url: Url, publisher: Publisher) {
        let key = channel_url.to_string();
        self.channel_publishers
            .entry(key)
            .or_default()
            .push(publisher);
    }

    /// Set the default publishers for unmapped channels.
    pub fn with_default_publishers(mut self, publishers: Vec<Publisher>) -> Self {
        self.default_publishers = Some(publishers);
        self
    }

    /// Set a custom signatures URL pattern.
    pub fn with_signatures_url_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.signatures_url_pattern = Some(pattern.into());
        self
    }

    /// Get the signatures URL for a package.
    ///
    /// If a custom `signatures_url_pattern` is configured, `{url}` is replaced
    /// with the package URL and the result is parsed; an invalid pattern result
    /// is reported as an error instead of silently falling back.
    ///
    /// Otherwise the default `.v0.sigs` suffix is appended to the package URL's
    /// *path* (preserving any query string), so package URLs carrying query
    /// strings (e.g. auth tokens) produce a correct signatures URL.
    pub fn signatures_url(&self, package_url: &Url) -> SigstoreResult<Url> {
        if let Some(pattern) = self.signatures_url_pattern.as_deref() {
            let url_str = pattern.replace("{url}", package_url.as_str());
            return Url::parse(&url_str).map_err(|e| SigstoreError::InvalidSignaturesUrl {
                url: url_str,
                message: e.to_string(),
            });
        }

        let mut url = package_url.clone();
        let sig_path = format!("{}.v0.sigs", url.path());
        url.set_path(&sig_path);
        Ok(url)
    }

    /// Find the publishers allowed for a given package URL.
    ///
    /// Channel prefixes are matched on URL origin and path-segment boundaries
    /// (not a raw string `starts_with`), so a prefix for `.../conda-forge` does
    /// not accidentally match `.../conda-forge-staging`. The most specific
    /// (longest by path-segment count) matching prefix wins.
    ///
    /// Returns `None` if the channel is not mapped and no default publishers are set.
    pub fn publishers_for_url(&self, package_url: &Url) -> Option<&[Publisher]> {
        // Find the most specific matching prefix (by path-segment count).
        let mut best_match: Option<(usize, &Vec<Publisher>)> = None;
        for (prefix, publishers) in &self.channel_publishers {
            let Ok(prefix_url) = Url::parse(prefix) else {
                continue;
            };
            if let Some(specificity) = url_prefix_specificity(&prefix_url, package_url) {
                match best_match {
                    Some((current, _)) if current >= specificity => {}
                    _ => best_match = Some((specificity, publishers)),
                }
            }
        }

        best_match
            .map(|(_, publishers)| publishers.as_slice())
            .or(self.default_publishers.as_deref())
    }
}

/// If `package_url` is located under the channel `prefix` URL, returns the
/// prefix's path-segment count (used as a specificity score). Returns `None`
/// when the package is not under the prefix.
///
/// Matching compares scheme/host/port and then requires the prefix's
/// non-empty path segments to be a prefix of the package's path segments, so
/// boundaries are respected (`conda-forge` does not match `conda-forge-staging`).
fn url_prefix_specificity(prefix: &Url, package_url: &Url) -> Option<usize> {
    if prefix.scheme() != package_url.scheme()
        || prefix.host_str() != package_url.host_str()
        || prefix.port_or_known_default() != package_url.port_or_known_default()
    {
        return None;
    }

    let prefix_segments: Vec<&str> = prefix
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let package_segments: Vec<&str> = package_url
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    if prefix_segments.len() > package_segments.len() {
        return None;
    }

    if prefix_segments
        .iter()
        .zip(&package_segments)
        .all(|(p, u)| p == u)
    {
        Some(prefix_segments.len())
    } else {
        None
    }
}

/// Verification policy for package signatures.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum VerificationPolicy {
    /// No signature verification is performed.
    #[default]
    Disabled,

    /// Signatures are verified, but failures only produce warnings.
    ///
    /// Packages will still be installed even if verification fails.
    Warn(VerificationConfig),

    /// Signatures must be valid for packages to be installed.
    ///
    /// If verification fails, the package will not be installed.
    Require(VerificationConfig),
}

impl VerificationPolicy {
    /// Check if verification is enabled.
    pub fn is_enabled(&self) -> bool {
        !matches!(self, VerificationPolicy::Disabled)
    }

    /// Check if verification failures should be treated as errors.
    pub fn is_required(&self) -> bool {
        matches!(self, VerificationPolicy::Require(_))
    }

    /// Get the verification configuration, if any.
    pub fn config(&self) -> Option<&VerificationConfig> {
        match self {
            VerificationPolicy::Disabled => None,
            VerificationPolicy::Warn(config) | VerificationPolicy::Require(config) => Some(config),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_publisher_matching() {
        let publisher = Publisher::new()
            .with_identity(
                "https://github.com/org/repo/.github/workflows/build.yaml@refs/heads/main",
            )
            .with_issuer("https://token.actions.githubusercontent.com");

        // Exact match
        assert!(publisher.matches(
            Some("https://github.com/org/repo/.github/workflows/build.yaml@refs/heads/main"),
            Some("https://token.actions.githubusercontent.com")
        ));

        // Wrong identity
        assert!(!publisher.matches(
            Some("https://github.com/other/repo/.github/workflows/build.yaml@refs/heads/main"),
            Some("https://token.actions.githubusercontent.com")
        ));

        // Wrong issuer
        assert!(!publisher.matches(
            Some("https://github.com/org/repo/.github/workflows/build.yaml@refs/heads/main"),
            Some("https://gitlab.com")
        ));
    }

    #[test]
    fn test_publisher_wildcard() {
        let publisher = Publisher::new()
            .with_identity("https://github.com/conda-forge/*")
            .with_issuer("https://token.actions.githubusercontent.com");

        // Matches prefix
        assert!(publisher.matches(
            Some("https://github.com/conda-forge/feedstock/.github/workflows/build.yaml@refs/heads/main"),
            Some("https://token.actions.githubusercontent.com")
        ));

        // Doesn't match different org
        assert!(!publisher.matches(
            Some("https://github.com/other-org/repo/.github/workflows/build.yaml@refs/heads/main"),
            Some("https://token.actions.githubusercontent.com")
        ));
    }

    #[test]
    fn test_config_longest_prefix() {
        let mut config = VerificationConfig::new();
        config.add_channel_publisher(
            Url::parse("https://conda.anaconda.org/").unwrap(),
            Publisher::new().with_issuer("default-issuer"),
        );
        config.add_channel_publisher(
            Url::parse("https://conda.anaconda.org/conda-forge/").unwrap(),
            Publisher::new().with_issuer("conda-forge-issuer"),
        );

        let pkg_url =
            Url::parse("https://conda.anaconda.org/conda-forge/linux-64/pkg.conda").unwrap();
        let publishers = config.publishers_for_url(&pkg_url).unwrap();
        assert_eq!(publishers[0].issuer.as_deref(), Some("conda-forge-issuer"));

        let other_url =
            Url::parse("https://conda.anaconda.org/bioconda/linux-64/pkg.conda").unwrap();
        let publishers = config.publishers_for_url(&other_url).unwrap();
        assert_eq!(publishers[0].issuer.as_deref(), Some("default-issuer"));
    }

    #[test]
    fn test_signatures_url_default_appends_to_path() {
        let config = VerificationConfig::new();
        let pkg_url =
            Url::parse("https://conda.anaconda.org/conda-forge/linux-64/pkg.conda").unwrap();
        assert_eq!(
            config.signatures_url(&pkg_url).unwrap().as_str(),
            "https://conda.anaconda.org/conda-forge/linux-64/pkg.conda.v0.sigs"
        );
    }

    #[test]
    fn test_signatures_url_preserves_query_string() {
        // The `.v0.sigs` suffix must be appended to the path, not the serialized
        // URL, so a query string (e.g. an auth token) stays intact.
        let config = VerificationConfig::new();
        let pkg_url =
            Url::parse("https://conda.anaconda.org/conda-forge/linux-64/pkg.conda?token=secret")
                .unwrap();
        assert_eq!(
            config.signatures_url(&pkg_url).unwrap().as_str(),
            "https://conda.anaconda.org/conda-forge/linux-64/pkg.conda.v0.sigs?token=secret"
        );
    }

    #[test]
    fn test_signatures_url_invalid_explicit_pattern_errors() {
        // An explicit but unparseable signatures URL must be a hard error rather
        // than silently falling back to a derived URL.
        let config = VerificationConfig::new().with_signatures_url_pattern("not a valid url");
        let pkg_url =
            Url::parse("https://conda.anaconda.org/conda-forge/linux-64/pkg.conda").unwrap();
        assert!(matches!(
            config.signatures_url(&pkg_url),
            Err(SigstoreError::InvalidSignaturesUrl { .. })
        ));
    }

    #[test]
    fn test_publishers_for_url_respects_segment_boundaries() {
        // A prefix for `conda-forge` must not match `conda-forge-staging`.
        let mut config = VerificationConfig::new();
        config.add_channel_publisher(
            Url::parse("https://conda.anaconda.org/conda-forge/").unwrap(),
            Publisher::new().with_issuer("conda-forge-issuer"),
        );

        let cf_url =
            Url::parse("https://conda.anaconda.org/conda-forge/linux-64/pkg.conda").unwrap();
        assert!(config.publishers_for_url(&cf_url).is_some());

        let staging_url =
            Url::parse("https://conda.anaconda.org/conda-forge-staging/linux-64/pkg.conda")
                .unwrap();
        // No default publishers, so a non-matching channel yields `None`.
        assert!(config.publishers_for_url(&staging_url).is_none());
    }

    #[test]
    fn test_publishers_for_url_matches_without_trailing_slash() {
        // A channel prefix configured without a trailing slash must still match
        // packages under that channel (boundary-aware, not raw `starts_with`).
        let mut config = VerificationConfig::new();
        config
            .channel_publishers
            .entry("https://conda.anaconda.org/conda-forge".to_string())
            .or_default()
            .push(Publisher::new().with_issuer("conda-forge-issuer"));

        let cf_url =
            Url::parse("https://conda.anaconda.org/conda-forge/linux-64/pkg.conda").unwrap();
        let publishers = config.publishers_for_url(&cf_url).unwrap();
        assert_eq!(publishers[0].issuer.as_deref(), Some("conda-forge-issuer"));
    }

    #[test]
    fn test_publishers_for_url_unmapped_without_default_is_none() {
        let config = VerificationConfig::new();
        let pkg_url =
            Url::parse("https://conda.anaconda.org/conda-forge/linux-64/pkg.conda").unwrap();
        assert!(config.publishers_for_url(&pkg_url).is_none());
    }
}
