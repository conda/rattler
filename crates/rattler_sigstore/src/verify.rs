//! Verification of attestation bundles against a package record.
//!
//! Verification has two layers:
//!
//! 1. Standard Sigstore verification of each bundle (signature, certificate
//!    chain, signed certificate timestamp, transparency log inclusion and the
//!    binding between the in-toto subject digest and the package SHA256),
//!    performed by `sigstore-verify` against the package record's `sha256`.
//!    The package archive itself is never needed.
//! 2. The CEP 27 checks on the in-toto statement: the predicate type must be
//!    the conda publish predicate, the subject name must equal the package
//!    filename and, depending on the [`ChannelCheck`], the `targetChannel`
//!    must match the channel the package was retrieved from.
//!
//! Which signing identities are trusted is decided afterwards by the
//! [`VerificationPolicy`].

use jiff::Timestamp;
use rattler_conda_types::RepoDataRecord;
use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;
use sigstore_types::{Artifact, Bundle, SignatureContent, intoto::Statement};
use sigstore_verify::{
    VerificationPolicy as SigstoreVerificationPolicy, Verifier, trust_root::TrustedRoot,
};
use tokio::sync::OnceCell;
use url::Url;

use crate::{
    error::{SigstoreError, SigstoreResult},
    policy::{ChannelCheck, Publisher, VerificationPolicy, normalize_channel_url},
    sidecar::{AttestationSidecar, fetch_sidecar},
};

/// The in-toto predicate type of a conda publish attestation (CEP 27).
pub const CONDA_PUBLISH_PREDICATE_TYPE: &str =
    "https://schemas.conda.org/attestations-publish-1.schema.json";

/// The predicate of a conda publish attestation (CEP 27).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CondaPublishPredicate {
    /// The channel the package was published to.
    #[serde(default)]
    pub target_channel: Option<String>,
}

/// An attestation that passed Sigstore verification and the CEP 27 checks.
#[derive(Debug, Clone)]
pub struct VerifiedAttestation {
    /// The index of the bundle in the sidecar.
    pub index: usize,
    /// The identity (SAN) of the signing certificate.
    pub identity: Option<String>,
    /// The OIDC issuer of the signing certificate.
    pub issuer: Option<String>,
    /// The time at which the signature was integrated into the transparency
    /// log.
    pub integrated_time: Option<Timestamp>,
    /// The `targetChannel` recorded in the attestation, if any.
    pub target_channel: Option<String>,
    /// Non-fatal observations made during verification.
    pub warnings: Vec<String>,
}

/// A bundle that did not pass verification.
#[derive(Debug, Clone)]
pub struct RejectedAttestation {
    /// The index of the bundle in the sidecar.
    pub index: usize,
    /// Why the bundle was rejected.
    pub reason: String,
}

/// The result of verifying every bundle of a sidecar.
#[derive(Debug, Clone, Default)]
pub struct BundleVerification {
    /// Bundles that passed.
    pub verified: Vec<VerifiedAttestation>,
    /// Bundles that failed.
    pub rejected: Vec<RejectedAttestation>,
}

/// Process-wide cache of the Sigstore production trusted root.
///
/// Loading the root performs network I/O (TUF), so it is done once per
/// process. Failures are not cached so a later call can retry.
static PRODUCTION_TRUSTED_ROOT: OnceCell<TrustedRoot> = OnceCell::const_new();

/// Returns the Sigstore production trusted root, fetching it through TUF on
/// first use.
pub async fn production_trusted_root() -> SigstoreResult<&'static TrustedRoot> {
    PRODUCTION_TRUSTED_ROOT
        .get_or_try_init(|| async {
            TrustedRoot::production()
                .await
                .map_err(|err| SigstoreError::TrustedRoot(err.to_string()))
        })
        .await
}

/// Verifies every bundle in `bundles` against `record` using `trusted_root`.
///
/// This performs Sigstore verification with the record's `sha256` as the
/// artifact digest followed by the CEP 27 statement checks. It does not apply
/// any trust policy: use the identities and issuers in the result for that.
pub fn verify_bundles(
    record: &RepoDataRecord,
    bundles: &[Bundle],
    channel_check: ChannelCheck,
    trusted_root: &TrustedRoot,
) -> SigstoreResult<BundleVerification> {
    let filename = record.identifier.to_file_name();
    let sha256 = record
        .package_record
        .sha256
        .as_ref()
        .ok_or_else(|| SigstoreError::MissingPackageSha256(filename.clone()))?;
    let artifact = Artifact::from_digest(sha256.as_slice());
    let expected_channel = expected_channel(record);

    let verifier = Verifier::new(trusted_root);
    let sigstore_policy = SigstoreVerificationPolicy::default();

    let mut result = BundleVerification::default();
    for (index, bundle) in bundles.iter().enumerate() {
        match verify_bundle(
            &verifier,
            &sigstore_policy,
            artifact.clone(),
            bundle,
            &filename,
            expected_channel.as_ref(),
            channel_check,
        ) {
            Ok(mut verified) => {
                verified.index = index;
                result.verified.push(verified);
            }
            Err(reason) => result.rejected.push(RejectedAttestation { index, reason }),
        }
    }
    Ok(result)
}

fn verify_bundle(
    verifier: &Verifier,
    sigstore_policy: &SigstoreVerificationPolicy,
    artifact: Artifact<'_>,
    bundle: &Bundle,
    filename: &str,
    expected_channel: Option<&Url>,
    channel_check: ChannelCheck,
) -> Result<VerifiedAttestation, String> {
    let outcome = verifier
        .verify(artifact, bundle, sigstore_policy)
        .map_err(|err| format!("sigstore verification failed: {err}"))?;

    let statement = statement_of(bundle)?;
    if statement.predicate_type != CONDA_PUBLISH_PREDICATE_TYPE {
        return Err(format!(
            "unexpected predicate type {:?}, expected {CONDA_PUBLISH_PREDICATE_TYPE:?}",
            statement.predicate_type
        ));
    }
    let Some(subject) = statement.subject.first() else {
        return Err("the in-toto statement has no subject".to_string());
    };
    if subject.name != filename {
        return Err(format!(
            "the attestation subject is {:?}, expected {filename:?}",
            subject.name
        ));
    }

    let predicate: CondaPublishPredicate = serde_json::from_value(statement.predicate)
        .map_err(|err| format!("invalid conda publish predicate: {err}"))?;

    let mut warnings = outcome.warnings;
    if let (Some(target_channel), Some(expected)) =
        (predicate.target_channel.as_deref(), expected_channel)
    {
        let matches = Url::parse(target_channel)
            .map(normalize_channel_url)
            .is_ok_and(|target| target == *expected);
        if !matches {
            let message = format!(
                "the attestation targets channel {target_channel:?} but the package was retrieved from {expected}"
            );
            match channel_check {
                ChannelCheck::Require => return Err(message),
                ChannelCheck::Warn => warnings.push(message),
                ChannelCheck::Ignore => {}
            }
        }
    }

    Ok(VerifiedAttestation {
        index: 0,
        identity: outcome.identity,
        issuer: outcome.issuer,
        integrated_time: outcome
            .integrated_time
            .and_then(|secs| Timestamp::from_second(secs).ok()),
        target_channel: predicate.target_channel,
        warnings,
    })
}

/// Extracts the in-toto statement from a DSSE bundle.
fn statement_of(bundle: &Bundle) -> Result<Statement, String> {
    let SignatureContent::DsseEnvelope(envelope) = &bundle.content else {
        return Err("the bundle is not a DSSE attestation".to_string());
    };
    serde_json::from_slice(&envelope.decode_payload())
        .map_err(|err| format!("the DSSE payload is not an in-toto statement: {err}"))
}

/// The channel a record was retrieved from, normalized for comparison with
/// `targetChannel`. Prefers the record's `channel` when it is a URL and falls
/// back to stripping `<subdir>/<filename>` from the package URL.
fn expected_channel(record: &RepoDataRecord) -> Option<Url> {
    if let Some(channel) = record.channel.as_deref()
        && let Ok(url) = Url::parse(channel)
    {
        return Some(normalize_channel_url(url));
    }
    let mut url = record.url.clone();
    url.path_segments_mut().ok()?.pop().pop();
    Some(normalize_channel_url(url))
}

/// The outcome of applying a [`VerificationPolicy`] to a record.
#[derive(Debug, Clone, Default)]
pub struct VerificationOutcome {
    /// The attestation that satisfied the policy, if any.
    pub attestation: Option<VerifiedAttestation>,
    /// Problems that did not block the installation because the policy is
    /// [`VerificationPolicy::Warn`], plus warnings from the accepted
    /// attestation.
    pub warnings: Vec<String>,
}

impl VerificationOutcome {
    /// Returns true if an attestation satisfied the policy.
    pub fn is_verified(&self) -> bool {
        self.attestation.is_some()
    }

    fn warning(message: String) -> Self {
        Self {
            attestation: None,
            warnings: vec![message],
        }
    }
}

/// Discovers, fetches and verifies the attestations of `record` according to
/// `policy`, using the Sigstore production trusted root.
///
/// In [`VerificationPolicy::Require`] mode every problem is an error. In
/// [`VerificationPolicy::Warn`] mode problems are returned as warnings and the
/// call only fails if the trusted root cannot be loaded.
pub async fn verify_record(
    policy: &VerificationPolicy,
    record: &RepoDataRecord,
    client: &ClientWithMiddleware,
) -> SigstoreResult<VerificationOutcome> {
    if !policy.is_enabled() {
        return Ok(VerificationOutcome::default());
    }
    let trusted_root = production_trusted_root().await?;
    verify_record_with_trusted_root(policy, record, client, trusted_root).await
}

/// Like [`verify_record`] but with an explicitly provided trusted root.
pub async fn verify_record_with_trusted_root(
    policy: &VerificationPolicy,
    record: &RepoDataRecord,
    client: &ClientWithMiddleware,
    trusted_root: &TrustedRoot,
) -> SigstoreResult<VerificationOutcome> {
    let Some(config) = policy.config() else {
        return Ok(VerificationOutcome::default());
    };
    let filename = record.identifier.to_file_name();

    let Some(publishers) = config.publishers_for_url(&record.url) else {
        if policy.is_required() {
            return Err(SigstoreError::NoPublishersConfigured(record.url.clone()));
        }
        tracing::debug!(
            "no attestation publishers configured for {}, skipping verification",
            record.url
        );
        return Ok(VerificationOutcome::default());
    };

    let sidecar = match fetch_sidecar(client, record, config.max_sidecar_size()).await {
        Ok(Some(sidecar)) => sidecar,
        Ok(None) => {
            let err = SigstoreError::NoAttestationsAdvertised(filename);
            return lenient(policy, err);
        }
        Err(err) => return lenient(policy, err),
    };

    let outcome = apply_policy(
        record,
        &sidecar,
        publishers,
        config.channel_check(),
        trusted_root,
    );
    match outcome {
        Ok(outcome) => Ok(outcome),
        Err(err) => lenient(policy, err),
    }
}

/// Verifies the bundles of `sidecar` and picks the first one that matches one
/// of `publishers`.
fn apply_policy(
    record: &RepoDataRecord,
    sidecar: &AttestationSidecar,
    publishers: &[Publisher],
    channel_check: ChannelCheck,
    trusted_root: &TrustedRoot,
) -> SigstoreResult<VerificationOutcome> {
    let verification = verify_bundles(record, &sidecar.bundles, channel_check, trusted_root)?;

    let mut reasons: Vec<String> = verification
        .rejected
        .iter()
        .map(|rejected| format!("bundle {}: {}", rejected.index, rejected.reason))
        .collect();

    for attestation in verification.verified {
        let accepted = publishers.is_empty()
            || publishers.iter().any(|publisher| {
                publisher.matches(
                    attestation.identity.as_deref(),
                    attestation.issuer.as_deref(),
                )
            });
        if accepted {
            tracing::debug!(
                "attestation {} of {} verified (identity: {:?}, issuer: {:?})",
                attestation.index,
                sidecar.url,
                attestation.identity,
                attestation.issuer
            );
            return Ok(VerificationOutcome {
                warnings: attestation.warnings.clone(),
                attestation: Some(attestation),
            });
        }
        reasons.push(format!(
            "bundle {}: valid signature by {} (issuer {}) does not match any trusted publisher",
            attestation.index,
            attestation
                .identity
                .as_deref()
                .unwrap_or("<unknown identity>"),
            attestation.issuer.as_deref().unwrap_or("<unknown issuer>"),
        ));
    }

    Err(SigstoreError::VerificationFailed {
        package: record.identifier.to_file_name(),
        reasons,
    })
}

/// Turns `err` into a warning outcome in `Warn` mode and propagates it in
/// `Require` mode.
fn lenient(policy: &VerificationPolicy, err: SigstoreError) -> SigstoreResult<VerificationOutcome> {
    if policy.is_required() {
        Err(err)
    } else {
        tracing::debug!("attestation verification problem downgraded to a warning: {err}");
        Ok(VerificationOutcome::warning(err.to_string()))
    }
}
