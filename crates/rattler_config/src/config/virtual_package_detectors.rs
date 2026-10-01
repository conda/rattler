//! Consent and limits for running virtual package detectors.
//!
//! A detector is a package a channel registers in its repodata; running it
//! executes code from that channel. Tools that ask the user before running a
//! detector store the answer here, keyed by the registering channel's canonical
//! base URL. The decision covers all its current and future detectors and their
//! resolved dependencies:
//!
//! ```toml
//! [virtual-package-detectors]
//! timeout-seconds = 30
//!
//! [virtual-package-detectors.consent]
//! "https://conda.anaconda.org/conda-forge" = "allow"
//! "https://prefix.dev/internal" = "deny"
//! ```

use std::time::Duration;

use indexmap::IndexMap;
use rattler_conda_types::{ChannelUrl, virtual_package_detector::MAX_TIMEOUT};
use serde::{Deserialize, Serialize};

use crate::config::{Config, MergeError, ValidationError};

/// A stored decision on whether a channel's detectors and their dependencies
/// may be installed and run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DetectorDecision {
    /// All detectors registered by the channel and their dependencies may be
    /// installed and run.
    Allow,
    /// No detector registered by the channel may run; its virtual packages are
    /// absent unless overridden.
    Deny,
}

/// The `virtual-package-detectors` section.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct VirtualPackageDetectorsConfig {
    /// How long a detector process may run, and separately how long its
    /// activation may take. Unset means the protocol default of 30 seconds;
    /// values above 300 seconds are rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,

    /// Consent decisions keyed by the registering channel's canonical base URL.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub consent: IndexMap<ChannelUrl, DetectorDecision>,
}

impl VirtualPackageDetectorsConfig {
    /// Whether nothing is configured.
    pub fn is_default(&self) -> bool {
        self.timeout_seconds.is_none() && self.consent.is_empty()
    }

    /// The configured timeout, if any.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout_seconds.map(Duration::from_secs)
    }

    /// The stored decision for a registering channel, if any.
    pub fn consent(&self, origin: &ChannelUrl) -> Option<DetectorDecision> {
        self.consent.get(origin).copied()
    }

    /// Records a decision for a registering channel, replacing an earlier one.
    pub fn set_consent(&mut self, origin: ChannelUrl, consent: DetectorDecision) {
        self.consent.insert(origin, consent);
    }
}

impl Config for VirtualPackageDetectorsConfig {
    /// `other` wins per key: its timeout replaces this one, and its consent
    /// entries replace the entries for the same channel while leaving the
    /// others in place.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        let mut consent = self.consent;
        consent.extend(
            other
                .consent
                .iter()
                .map(|(origin, decision)| (origin.clone(), *decision)),
        );
        Ok(Self {
            timeout_seconds: other.timeout_seconds.or(self.timeout_seconds),
            consent,
        })
    }

    fn validate(&self) -> Result<(), ValidationError> {
        if self.timeout_seconds == Some(0) {
            return Err(ValidationError::InvalidValue(
                "virtual-package-detectors.timeout-seconds".to_string(),
                "must be at least 1 second".to_string(),
            ));
        }
        if let Some(seconds) = self.timeout_seconds
            && seconds > MAX_TIMEOUT.as_secs()
        {
            return Err(ValidationError::InvalidValue(
                "virtual-package-detectors.timeout-seconds".to_string(),
                format!(
                    "must be at most {} seconds, got {seconds}",
                    MAX_TIMEOUT.as_secs()
                ),
            ));
        }
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec!["timeout-seconds".to_string(), "consent".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> VirtualPackageDetectorsConfig {
        toml::from_str(toml).unwrap()
    }

    fn origin(url: &str) -> ChannelUrl {
        ChannelUrl::from(url::Url::parse(url).unwrap())
    }

    #[test]
    fn parses_consent_and_timeout() {
        let config = parse(
            r#"
timeout-seconds = 60

[consent]
"https://conda.anaconda.org/conda-forge" = "allow"
"https://prefix.dev/internal" = "deny"
"#,
        );
        assert_eq!(config.timeout(), Some(Duration::from_secs(60)));
        let forge = origin("https://conda.anaconda.org/conda-forge");
        assert_eq!(config.consent(&forge), Some(DetectorDecision::Allow));
        assert_eq!(
            config.consent(&origin("https://prefix.dev/internal")),
            Some(DetectorDecision::Deny)
        );
        assert_eq!(config.consent(&origin("https://example.com/unknown")), None);
        // Origins normalize their trailing slash, so both spellings match.
        assert_eq!(
            config.consent(&origin("https://conda.anaconda.org/conda-forge/")),
            Some(DetectorDecision::Allow)
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let mut config = VirtualPackageDetectorsConfig::default();
        assert!(config.is_default());
        config.set_consent(
            origin("https://conda.anaconda.org/conda-forge"),
            DetectorDecision::Allow,
        );
        config.timeout_seconds = Some(45);
        let rendered = toml::to_string(&config).unwrap();
        assert_eq!(parse(&rendered), config);
    }

    #[test]
    fn merge_replaces_per_channel() {
        let base = parse(
            r#"
timeout-seconds = 10

[consent]
"https://conda.anaconda.org/conda-forge" = "deny"
"https://example.com/retained" = "deny"
"#,
        );
        let other = parse(
            r#"
[consent]
"https://conda.anaconda.org/conda-forge/" = "allow"
"https://prefix.dev/internal" = "allow"
"#,
        );
        let merged = base.merge_config(&other).unwrap();
        assert_eq!(merged.timeout_seconds, Some(10));
        let forge = origin("https://conda.anaconda.org/conda-forge");
        assert_eq!(merged.consent(&forge), Some(DetectorDecision::Allow));
        assert_eq!(
            merged.consent(&origin("https://example.com/retained")),
            Some(DetectorDecision::Deny)
        );
        assert_eq!(
            merged.consent(&origin("https://prefix.dev/internal")),
            Some(DetectorDecision::Allow)
        );
    }

    #[test]
    fn rejects_timeouts_above_the_ceiling() {
        assert!(parse("timeout-seconds = 300").validate().is_ok());
        assert!(parse("timeout-seconds = 0").validate().is_err());
        assert!(parse("timeout-seconds = 301").validate().is_err());
    }

    #[test]
    fn rejects_unknown_decisions() {
        let result = toml::from_str::<VirtualPackageDetectorsConfig>(
            r#"
[consent]
"https://conda.anaconda.org/conda-forge" = "maybe"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_detector_scoped_consent() {
        let result = toml::from_str::<VirtualPackageDetectorsConfig>(
            r#"
[consent."https://conda.anaconda.org/conda-forge"]
mpi-detect = "allow"
"#,
        );
        assert!(result.is_err());
    }
}
