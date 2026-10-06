//! Whether a detector may run.
//!
//! Running a detector executes code from a channel. The channel registration
//! CEP treats configuring a channel as consent, and lets a client require more.
//! [`DetectorConsent`] is where a client plugs that in: it is asked once per
//! detector that would otherwise run, after the detector environment has been
//! resolved, so it can show what would be installed. Requests for different
//! detectors may arrive concurrently, including detectors from the same channel.
//! Policies that ask for channel-wide trust must deduplicate and retain their
//! answer by the registering channel's canonical base URL.

use async_trait::async_trait;
use rattler_conda_types::{
    Channel, RepoDataRecord, virtual_package_detector::DetectorRegistration,
};
use rattler_config::config::virtual_package_detectors::{
    DetectorDecision, VirtualPackageDetectorsConfig,
};
use rattler_digest::Sha256Hash;

/// The decision on a [`ConsentRequest`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consent {
    /// Install and run the detector.
    Allow,
    /// Do not run the detector; its names are absent unless overridden.
    Deny,
}

/// Everything a policy may want to show or check before deciding.
#[derive(Debug)]
pub struct ConsentRequest<'a> {
    /// The channel that registered the detector.
    pub channel: &'a Channel,
    /// The detector and the virtual packages it reports.
    pub registration: &'a DetectorRegistration,
    /// The channels the detector was resolved against.
    pub resolution_channels: &'a [Channel],
    /// The packages that would be installed, the detector included.
    pub records: &'a [RepoDataRecord],
    /// The environment digest of `records`.
    pub digest: Sha256Hash,
}

/// A policy deciding whether detectors may run.
#[async_trait]
pub trait DetectorConsent: Send + Sync {
    /// Decides on one detector after it was resolved, with everything a
    /// [`ConsentRequest`] carries at hand.
    async fn decide(&self, request: &ConsentRequest<'_>) -> Consent;

    /// A decision that needs nothing but the registration, if the policy has
    /// one. A denial here spares resolving the detector at all.
    fn decide_before_resolving(
        &self,
        _channel: &Channel,
        _registration: &DetectorRegistration,
    ) -> Option<Consent> {
        None
    }

    /// Whether this policy can ever answer [`Consent::Allow`]. A client may
    /// skip reading registrations for a policy that cannot.
    fn can_allow(&self) -> bool {
        true
    }
}

#[async_trait]
impl<T: DetectorConsent + ?Sized> DetectorConsent for std::sync::Arc<T> {
    async fn decide(&self, request: &ConsentRequest<'_>) -> Consent {
        (**self).decide(request).await
    }

    fn decide_before_resolving(
        &self,
        channel: &Channel,
        registration: &DetectorRegistration,
    ) -> Option<Consent> {
        (**self).decide_before_resolving(channel, registration)
    }

    fn can_allow(&self) -> bool {
        (**self).can_allow()
    }
}

/// Runs every detector. This is the channel registration CEP's baseline, where
/// configuring the channel is the consent.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAll;

#[async_trait]
impl DetectorConsent for AllowAll {
    async fn decide(&self, _request: &ConsentRequest<'_>) -> Consent {
        Consent::Allow
    }

    fn decide_before_resolving(
        &self,
        _channel: &Channel,
        _registration: &DetectorRegistration,
    ) -> Option<Consent> {
        Some(Consent::Allow)
    }
}

/// Runs no detector.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAll;

#[async_trait]
impl DetectorConsent for DenyAll {
    async fn decide(&self, _request: &ConsentRequest<'_>) -> Consent {
        Consent::Deny
    }

    fn decide_before_resolving(
        &self,
        _channel: &Channel,
        _registration: &DetectorRegistration,
    ) -> Option<Consent> {
        Some(Consent::Deny)
    }

    fn can_allow(&self) -> bool {
        false
    }
}

/// Decides from the stored decisions in the `virtual-package-detectors`
/// configuration and defers to `fallback` for channels without one. A channel's
/// decision covers all detectors it registers and their resolved dependencies.
pub struct ConfiguredConsent<F> {
    config: VirtualPackageDetectorsConfig,
    fallback: F,
}

impl<F: DetectorConsent> ConfiguredConsent<F> {
    /// Uses the decisions in `config`, asking `fallback` for the rest.
    pub fn new(config: VirtualPackageDetectorsConfig, fallback: F) -> Self {
        Self { config, fallback }
    }

    /// The stored decision for a registering channel, if any.
    pub fn stored(&self, channel: &Channel) -> Option<Consent> {
        self.config
            .consent(&channel.base_url)
            .map(|stored| match stored {
                DetectorDecision::Allow => Consent::Allow,
                DetectorDecision::Deny => Consent::Deny,
            })
    }
}

#[async_trait]
impl<F: DetectorConsent> DetectorConsent for ConfiguredConsent<F> {
    async fn decide(&self, request: &ConsentRequest<'_>) -> Consent {
        match self.stored(request.channel) {
            Some(consent) => consent,
            None => self.fallback.decide(request).await,
        }
    }

    fn decide_before_resolving(
        &self,
        channel: &Channel,
        registration: &DetectorRegistration,
    ) -> Option<Consent> {
        self.stored(channel)
            .or_else(|| self.fallback.decide_before_resolving(channel, registration))
    }

    fn can_allow(&self) -> bool {
        self.fallback.can_allow()
            || self
                .config
                .consent
                .values()
                .any(|decision| *decision == DetectorDecision::Allow)
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexSet;
    use rattler_conda_types::{PackageName, virtual_package_detector::VirtualPackageName};
    use url::Url;

    use super::*;

    fn channel(url: &str) -> Channel {
        Channel::from_url(Url::parse(url).unwrap())
    }

    fn registration(detector: &str, virtual_package: &str) -> DetectorRegistration {
        DetectorRegistration {
            detector: PackageName::try_from(detector).unwrap(),
            virtual_packages: IndexSet::from([
                VirtualPackageName::try_from(virtual_package).unwrap()
            ]),
        }
    }

    fn request<'a>(
        channel: &'a Channel,
        registration: &'a DetectorRegistration,
    ) -> ConsentRequest<'a> {
        ConsentRequest {
            channel,
            registration,
            resolution_channels: &[],
            records: &[],
            digest: Sha256Hash::default(),
        }
    }

    #[tokio::test]
    async fn channel_allow_applies_to_current_and_future_detectors() {
        let forge = channel("https://conda.anaconda.org/conda-forge");
        let other = channel("https://prefix.dev/internal");
        let existing = registration("mpi-detect", "__mpi");
        let mut config = VirtualPackageDetectorsConfig::default();
        config.set_consent(forge.base_url.clone(), DetectorDecision::Allow);
        let policy = ConfiguredConsent::new(config, DenyAll);
        let newly_registered = registration("cuda-detect", "__cuda");

        for detector in [&existing, &newly_registered] {
            assert_eq!(
                policy.decide(&request(&forge, detector)).await,
                Consent::Allow
            );
            assert_eq!(
                policy.decide_before_resolving(&forge, detector),
                Some(Consent::Allow)
            );
            assert_eq!(
                policy.decide(&request(&other, detector)).await,
                Consent::Deny
            );
        }
        assert_eq!(policy.stored(&other), None);
        assert!(policy.can_allow());
    }

    #[tokio::test]
    async fn channel_deny_overrides_fallback_before_resolving() {
        let forge = channel("https://conda.anaconda.org/conda-forge");
        let other = channel("https://prefix.dev/internal");
        let mut config = VirtualPackageDetectorsConfig::default();
        config.set_consent(forge.base_url.clone(), DetectorDecision::Deny);
        let policy = ConfiguredConsent::new(config, AllowAll);

        for detector in [
            registration("mpi-detect", "__mpi"),
            registration("cuda-detect", "__cuda"),
        ] {
            assert_eq!(
                policy.decide_before_resolving(&forge, &detector),
                Some(Consent::Deny)
            );
            assert_eq!(
                policy.decide(&request(&forge, &detector)).await,
                Consent::Deny
            );
            assert_eq!(
                policy.decide(&request(&other, &detector)).await,
                Consent::Allow
            );
        }
        assert!(policy.can_allow());
    }

    #[test]
    fn can_allow_requires_a_stored_allow_or_a_permissive_fallback() {
        let forge = channel("https://conda.anaconda.org/conda-forge");
        let other = channel("https://prefix.dev/internal");
        let mut config = VirtualPackageDetectorsConfig::default();
        assert!(!ConfiguredConsent::new(config.clone(), DenyAll).can_allow());
        config.set_consent(forge.base_url.clone(), DetectorDecision::Deny);
        assert!(!ConfiguredConsent::new(config.clone(), DenyAll).can_allow());
        assert!(ConfiguredConsent::new(config.clone(), AllowAll).can_allow());
        config.set_consent(other.base_url, DetectorDecision::Allow);
        assert!(ConfiguredConsent::new(config, DenyAll).can_allow());
    }
}
