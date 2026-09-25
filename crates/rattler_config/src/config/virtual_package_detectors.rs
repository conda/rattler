//! Consent and limits for running virtual package detectors.
//!
//! A detector is a package a channel registers in its repodata; running it
//! executes code from that channel. Tools that ask the user before running a
//! detector store the answer here, keyed by the registration's origin (the
//! registering channel's base URL) and the detector's package name, so every
//! rattler-based tool honors the same decision:
//!
//! ```toml
//! [virtual-package-detectors]
//! timeout-seconds = 30
//!
//! [virtual-package-detectors.consent."https://conda.anaconda.org/conda-forge"]
//! mpi-detect = "allow"
//! cuda-detect = "deny"
//! ```

use std::time::Duration;

use indexmap::IndexMap;
use rattler_conda_types::{ChannelUrl, PackageName, virtual_package_detector::MAX_TIMEOUT};
use serde::{Deserialize, Serialize};

use crate::config::{Config, MergeError, ValidationError};

/// A stored decision on whether a detector may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DetectorDecision {
    /// The detector may be installed and run.
    Allow,
    /// The detector must not run; its virtual packages are absent.
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

    /// Consent decisions keyed by registration origin, then detector name.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub consent: IndexMap<ChannelUrl, IndexMap<PackageName, DetectorDecision>>,
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

    /// The stored decision for a detector, if any.
    pub fn consent(&self, origin: &ChannelUrl, detector: &PackageName) -> Option<DetectorDecision> {
        self.consent.get(origin)?.get(detector).copied()
    }

    /// Records a decision for a detector, replacing an earlier one.
    pub fn set_consent(
        &mut self,
        origin: ChannelUrl,
        detector: PackageName,
        consent: DetectorDecision,
    ) {
        self.consent
            .entry(origin)
            .or_default()
            .insert(detector, consent);
    }
}

impl Config for VirtualPackageDetectorsConfig {
    /// `other` wins per key: its timeout replaces this one, and its consent
    /// entries replace the entries for the same origin and detector while
    /// leaving the others in place.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        let mut consent = self.consent;
        for (origin, detectors) in &other.consent {
            consent.entry(origin.clone()).or_default().extend(
                detectors
                    .iter()
                    .map(|(name, decision)| (name.clone(), *decision)),
            );
        }
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

    fn name(name: &str) -> PackageName {
        PackageName::try_from(name).unwrap()
    }

    #[test]
    fn parses_consent_and_timeout() {
        let config = parse(
            r#"
timeout-seconds = 60

[consent."https://conda.anaconda.org/conda-forge"]
mpi-detect = "allow"
cuda-detect = "deny"
"#,
        );
        assert_eq!(config.timeout(), Some(Duration::from_secs(60)));
        let forge = origin("https://conda.anaconda.org/conda-forge");
        assert_eq!(
            config.consent(&forge, &name("mpi-detect")),
            Some(DetectorDecision::Allow)
        );
        assert_eq!(
            config.consent(&forge, &name("cuda-detect")),
            Some(DetectorDecision::Deny)
        );
        assert_eq!(config.consent(&forge, &name("other")), None);
        // Origins normalize their trailing slash, so both spellings match.
        assert_eq!(
            config.consent(
                &origin("https://conda.anaconda.org/conda-forge/"),
                &name("mpi-detect")
            ),
            Some(DetectorDecision::Allow)
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let mut config = VirtualPackageDetectorsConfig::default();
        assert!(config.is_default());
        config.set_consent(
            origin("https://conda.anaconda.org/conda-forge"),
            name("mpi-detect"),
            DetectorDecision::Allow,
        );
        config.timeout_seconds = Some(45);
        let rendered = toml::to_string(&config).unwrap();
        insta::assert_snapshot!(rendered, @r#"
        timeout-seconds = 45

        [consent."https://conda.anaconda.org/conda-forge"]
        mpi-detect = "allow"
        "#);
        assert_eq!(parse(&rendered), config);
    }

    #[test]
    fn merge_replaces_per_detector() {
        let base = parse(
            r#"
timeout-seconds = 10

[consent."https://conda.anaconda.org/conda-forge"]
mpi-detect = "deny"
cuda-detect = "deny"
"#,
        );
        let other = parse(
            r#"
[consent."https://conda.anaconda.org/conda-forge"]
mpi-detect = "allow"

[consent."https://prefix.dev/internal"]
site-detect = "allow"
"#,
        );
        let merged = base.merge_config(&other).unwrap();
        assert_eq!(merged.timeout_seconds, Some(10));
        let forge = origin("https://conda.anaconda.org/conda-forge");
        assert_eq!(
            merged.consent(&forge, &name("mpi-detect")),
            Some(DetectorDecision::Allow)
        );
        assert_eq!(
            merged.consent(&forge, &name("cuda-detect")),
            Some(DetectorDecision::Deny)
        );
        assert_eq!(
            merged.consent(&origin("https://prefix.dev/internal"), &name("site-detect")),
            Some(DetectorDecision::Allow)
        );
    }

    #[test]
    fn rejects_timeouts_above_the_ceiling() {
        assert!(parse("timeout-seconds = 300").validate().is_ok());
        assert!(parse("timeout-seconds = 0").validate().is_err());
        let err = parse("timeout-seconds = 301").validate().unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid value for field virtual-package-detectors.timeout-seconds: must be at most 300 seconds, got 301"
        );
    }

    #[test]
    fn rejects_unknown_decisions() {
        let err = toml::from_str::<VirtualPackageDetectorsConfig>(
            r#"
[consent."https://conda.anaconda.org/conda-forge"]
mpi-detect = "maybe"
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown variant `maybe`"), "{err}");
    }
}
