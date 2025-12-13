//! Verification policy types and configuration.

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
    /// `Require` mode or be skipped in `Warn` mode.
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
    pub fn signatures_url(&self, package_url: &Url) -> Url {
        let pattern = self
            .signatures_url_pattern
            .as_deref()
            .unwrap_or("{url}.v0.sigs");
        let url_str = pattern.replace("{url}", package_url.as_str());
        // This should not fail if the pattern is valid
        Url::parse(&url_str).unwrap_or_else(|_| {
            let mut url = package_url.clone();
            url.set_path(&format!("{}.v0.sigs", url.path()));
            url
        })
    }

    /// Find the publishers allowed for a given package URL.
    ///
    /// Returns `None` if the channel is not mapped and no default publishers are set.
    pub fn publishers_for_url(&self, package_url: &Url) -> Option<&[Publisher]> {
        let url_str = package_url.as_str();

        // Find the longest matching prefix
        let mut best_match: Option<(&str, &Vec<Publisher>)> = None;
        for (prefix, publishers) in &self.channel_publishers {
            if url_str.starts_with(prefix) {
                match best_match {
                    None => best_match = Some((prefix.as_str(), publishers)),
                    Some((current_prefix, _)) if prefix.len() > current_prefix.len() => {
                        best_match = Some((prefix.as_str(), publishers));
                    }
                    _ => {}
                }
            }
        }

        best_match
            .map(|(_, publishers)| publishers.as_slice())
            .or(self.default_publishers.as_deref())
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
}
