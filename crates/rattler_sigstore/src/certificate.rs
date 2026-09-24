//! The claims a Fulcio signing certificate makes about the CI workload that
//! produced an attestation.
//!
//! Sigstore records the workload identity of the signer in X.509 extensions
//! under the Fulcio OID arc `1.3.6.1.4.1.57264.1`. Only the OIDC issuer
//! (`.1.1`) and the Subject Alternative Name are interpreted by
//! `sigstore-verify`; the remaining extensions describe *where the artifact
//! came from* — repository, commit, workflow file and the CI run that invoked
//! the signer — and are parsed here.
//!
//! The extension names are deliberately provider-neutral: GitHub Actions,
//! GitLab CI and Buildkite all populate the same arc from their respective
//! workload identity tokens, so consumers should render whichever claims are
//! present rather than branching on the issuer.
//!
//! Nothing in this module is specific to conda, so it belongs upstream next to
//! `sigstore_crypto::x509::CertificateInfo` rather than here. It lives in this
//! crate until `sigstore-rust` grows an equivalent, at which point
//! [`CertificateClaims`] can become a re-export: the field names are taken
//! verbatim from the Fulcio specification so that swapping the implementation
//! does not change this crate's API.

use jiff::Timestamp;
use serde::Serialize;
use sigstore_types::TimeRange;
use x509_cert::{
    Certificate,
    der::{Decode, asn1::ObjectIdentifier, asn1::Utf8StringRef},
};

use crate::error::{SigstoreError, SigstoreResult};

/// The Fulcio OID arc under which the CI claims live.
const FULCIO_CLAIM_ARC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.4.1.57264.1");

/// Returns the trailing component of `oid` if it is a direct child of
/// [`FULCIO_CLAIM_ARC`], i.e. the number that identifies the claim.
fn claim_number(oid: &ObjectIdentifier) -> Option<u32> {
    if oid.parent()? != FULCIO_CLAIM_ARC {
        return None;
    }
    oid.arc(FULCIO_CLAIM_ARC.len())
}

/// A signing certificate that passed Sigstore verification.
///
/// The claims are only meaningful because the certificate chain, its validity
/// period and its signed certificate timestamp were verified first; a
/// [`SigningCertificate`] is therefore only ever produced for a bundle that
/// passed verification.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SigningCertificate {
    /// The period the certificate is valid for.
    ///
    /// Fulcio issues short-lived certificates, so `start` is within seconds of
    /// the moment the artifact was signed and is the best available answer to
    /// "when was this signed?".
    pub validity: TimeRange,

    /// The CI claims the certificate asserts.
    pub claims: CertificateClaims,
}

impl SigningCertificate {
    /// Parses a DER-encoded X.509 certificate.
    pub fn from_der(der: &[u8]) -> SigstoreResult<Self> {
        let certificate = Certificate::from_der(der).map_err(|err| {
            SigstoreError::InvalidCertificate(format!("failed to decode the certificate: {err}"))
        })?;
        let validity = &certificate.tbs_certificate.validity;
        let not_before = timestamp(validity.not_before.to_system_time(), "notBefore")?;
        let not_after = timestamp(validity.not_after.to_system_time(), "notAfter")?;

        Ok(Self {
            validity: TimeRange::new(not_before, Some(not_after)),
            claims: CertificateClaims::from_certificate(&certificate),
        })
    }

    /// The moment the certificate became valid, which approximates the signing
    /// time.
    pub fn issued_at(&self) -> Timestamp {
        self.validity.start
    }
}

fn timestamp(time: std::time::SystemTime, field: &str) -> SigstoreResult<Timestamp> {
    Timestamp::try_from(time).map_err(|err| {
        SigstoreError::InvalidCertificate(format!("the certificate has an invalid {field}: {err}"))
    })
}

/// The CI claims of a Fulcio signing certificate.
///
/// Every claim is optional: Fulcio only sets them for certificates issued to a
/// CI workload, and which of them are set depends on the identity provider. A
/// certificate issued to a human identity (e.g. through an interactive OIDC
/// flow) carries none of them.
///
/// The deprecated `1.3.6.1.4.1.57264.1.2` to `.1.6` extensions are not parsed;
/// the claims below supersede them and are DER encoded rather than bare
/// strings.
///
/// Claims serialize with every key present, absent ones as `null`, so that
/// consumers see a stable set of fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct CertificateClaims {
    /// `.1.9` — the workflow or pipeline that requested the signature,
    /// including its ref. This is the value the certificate's SAN identity is
    /// derived from.
    pub build_signer_uri: Option<String>,

    /// `.1.10` — the commit the signing workflow itself was taken from.
    pub build_signer_digest: Option<String>,

    /// `.1.11` — the kind of runner the workload ran on, e.g. `github-hosted`
    /// or `self-hosted`.
    ///
    /// A self-hosted runner is a weaker guarantee than a provider-hosted one,
    /// because its environment is controlled by the repository owner.
    pub runner_environment: Option<String>,

    /// `.1.12` — the repository the artifact was built from.
    pub source_repository_uri: Option<String>,

    /// `.1.13` — the commit the artifact was built from.
    pub source_repository_digest: Option<String>,

    /// `.1.14` — the git ref the artifact was built from, e.g.
    /// `refs/heads/main` or `refs/tags/v1.0.0`.
    pub source_repository_ref: Option<String>,

    /// `.1.15` — the provider's immutable identifier for the repository.
    ///
    /// Unlike [`Self::source_repository_uri`] this does not change when the
    /// repository is renamed or transferred, which makes it the more robust
    /// value to pin a trust policy to.
    pub source_repository_identifier: Option<String>,

    /// `.1.16` — the owner of the source repository.
    pub source_repository_owner_uri: Option<String>,

    /// `.1.17` — the provider's immutable identifier for the repository owner.
    pub source_repository_owner_identifier: Option<String>,

    /// `.1.18` — the build configuration that ran, e.g. the workflow file.
    pub build_config_uri: Option<String>,

    /// `.1.19` — the commit the build configuration was taken from.
    pub build_config_digest: Option<String>,

    /// `.1.20` — the event that triggered the build, e.g. `push`, `release` or
    /// `workflow_dispatch`.
    pub build_trigger: Option<String>,

    /// `.1.21` — the specific CI run that produced the signature.
    ///
    /// For GitHub Actions this is a URL to the workflow run, which makes the
    /// build logs directly inspectable.
    pub run_invocation_uri: Option<String>,

    /// `.1.22` — whether the source repository was `public`, `private` or
    /// `internal` when the certificate was issued.
    ///
    /// A signature from a private repository cannot be audited by a third
    /// party, since neither the source nor the build logs are reachable.
    pub source_repository_visibility_at_signing: Option<String>,
}

impl CertificateClaims {
    /// Parses the claims from a DER-encoded X.509 certificate.
    ///
    /// Extensions that are absent or malformed are left unset rather than
    /// reported as an error, so that a certificate from a provider that only
    /// populates part of the arc still yields the claims it does carry.
    pub fn from_der(der: &[u8]) -> SigstoreResult<Self> {
        let certificate = Certificate::from_der(der).map_err(|err| {
            SigstoreError::InvalidCertificate(format!("failed to decode the certificate: {err}"))
        })?;
        Ok(Self::from_certificate(&certificate))
    }

    /// Returns true if the certificate carried no CI claims at all.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn from_certificate(certificate: &Certificate) -> Self {
        let Some(extensions) = certificate.tbs_certificate.extensions.as_ref() else {
            return Self::default();
        };

        let mut claims = Self::default();
        for extension in extensions {
            let Some(number) = claim_number(&extension.extn_id) else {
                continue;
            };
            let target = match number {
                9 => &mut claims.build_signer_uri,
                10 => &mut claims.build_signer_digest,
                11 => &mut claims.runner_environment,
                12 => &mut claims.source_repository_uri,
                13 => &mut claims.source_repository_digest,
                14 => &mut claims.source_repository_ref,
                15 => &mut claims.source_repository_identifier,
                16 => &mut claims.source_repository_owner_uri,
                17 => &mut claims.source_repository_owner_identifier,
                18 => &mut claims.build_config_uri,
                19 => &mut claims.build_config_digest,
                20 => &mut claims.build_trigger,
                21 => &mut claims.run_invocation_uri,
                22 => &mut claims.source_repository_visibility_at_signing,
                // `.1.1` is the OIDC issuer, which `sigstore-verify` already
                // reports, and `.1.2` to `.1.6` are the deprecated claims.
                _ => continue,
            };

            // The value of every claim in this arc is a DER encoded UTF8String
            // wrapped in the extension's OCTET STRING.
            if let Ok(value) = Utf8StringRef::from_der(extension.extn_value.as_bytes()) {
                *target = Some(value.as_str().to_owned());
            }
        }
        claims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sidecar of a package published from a GitHub Actions workflow, whose
    /// signing certificate carries the full claim arc.
    const SIDECAR: &[u8] = include_bytes!("../test-data/actionlint-1.7.12-h60d57d3_0.conda.sigs");

    fn github_certificate() -> SigningCertificate {
        let bundles: Vec<sigstore_types::Bundle> = serde_json::from_slice(SIDECAR).unwrap();
        let der = bundles[0]
            .signing_certificate()
            .expect("the bundle is signed with a Fulcio certificate");
        SigningCertificate::from_der(der.as_bytes()).unwrap()
    }

    #[test]
    fn parses_the_github_actions_claims() {
        let claims = github_certificate().claims;

        assert_eq!(
            claims.source_repository_uri.as_deref(),
            Some("https://github.com/hunger/octoconda")
        );
        assert_eq!(
            claims.source_repository_digest.as_deref(),
            Some("3486769623122fdddade1fb57e1b4728f162608a")
        );
        assert_eq!(
            claims.source_repository_ref.as_deref(),
            Some("refs/heads/main")
        );
        assert_eq!(
            claims.source_repository_identifier.as_deref(),
            Some("1092908474")
        );
        assert_eq!(
            claims.source_repository_owner_uri.as_deref(),
            Some("https://github.com/hunger")
        );
        assert_eq!(
            claims.source_repository_owner_identifier.as_deref(),
            Some("73267")
        );
        assert_eq!(
            claims.source_repository_visibility_at_signing.as_deref(),
            Some("public")
        );
        assert_eq!(claims.runner_environment.as_deref(), Some("github-hosted"));
        assert_eq!(claims.build_trigger.as_deref(), Some("schedule"));
        assert_eq!(
            claims.build_config_uri.as_deref(),
            Some(
                "https://github.com/hunger/octoconda/.github/workflows/octoconda.yaml@refs/heads/main"
            )
        );
        assert_eq!(
            claims.build_config_digest.as_deref(),
            Some("3486769623122fdddade1fb57e1b4728f162608a")
        );
        assert_eq!(
            claims.run_invocation_uri.as_deref(),
            Some("https://github.com/hunger/octoconda/actions/runs/23778256205/attempts/1")
        );
        assert!(!claims.is_empty());
    }

    #[test]
    fn parses_the_short_lived_validity_window() {
        let certificate = github_certificate();
        let end = certificate
            .validity
            .end
            .expect("a Fulcio certificate always expires");
        // Fulcio issues certificates with a ten minute lifetime.
        assert_eq!(
            end.duration_since(certificate.issued_at()),
            jiff::SignedDuration::from_mins(10)
        );
    }

    #[test]
    fn rejects_bytes_that_are_not_a_certificate() {
        assert!(matches!(
            SigningCertificate::from_der(b"not a certificate"),
            Err(SigstoreError::InvalidCertificate(_))
        ));
    }
}
