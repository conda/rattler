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
use rattler_redaction::Redact;
use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;
use sigstore_types::{Artifact, Bundle, SignatureContent, TransparencyLogEntry, intoto::Subject};
use sigstore_verify::{
    VerificationPolicy as SigstoreVerificationPolicy, VerificationResult, Verifier,
    trust_root::{SigstoreInstance, TrustedRoot},
};
use tokio::sync::OnceCell;
use url::Url;

use crate::{
    certificate::{CertificateClaims, SigningCertificate},
    error::{SigstoreError, SigstoreResult},
    policy::{ChannelCheck, Publisher, VerificationPolicy, normalize_channel_url},
    sidecar::{AttestationSidecar, fetch_sidecar},
};

/// The in-toto predicate type of a conda publish attestation (CEP 27).
pub const CONDA_PUBLISH_PREDICATE_TYPE: &str =
    "https://schemas.conda.org/attestations-publish-1.schema.json";

const IN_TOTO_STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CondaPublishStatement {
    #[serde(rename = "_type")]
    type_: String,
    subject: Vec<Subject>,
    predicate_type: String,
    #[serde(default)]
    predicate: Option<CondaPublishPredicate>,
}

/// The predicate of a conda publish attestation (CEP 27).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CondaPublishPredicate {
    /// The channel the package was published to.
    pub target_channel: String,
}

/// An attestation that passed Sigstore verification and the CEP 27 checks.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct VerifiedAttestation {
    /// The index of the bundle in the sidecar.
    pub index: usize,
    /// The identity (SAN) of the signing certificate.
    pub identity: Option<String>,
    /// The OIDC issuer of the signing certificate.
    pub issuer: Option<String>,
    /// The authenticated time at which the signature was integrated into the
    /// transparency log.
    ///
    /// Unlike [`TransparencyLogEntry::integrated_time`] of [`Self::log_entry`],
    /// which is an unverified claim of the bundle, this is only set when log
    /// inclusion was actually verified.
    pub integrated_time: Option<Timestamp>,
    /// The `targetChannel` recorded in the attestation, if any.
    pub target_channel: Option<String>,
    /// The verified signing certificate: its validity window and the claims it
    /// makes about the CI workload that signed the package.
    ///
    /// This is `None` for a bundle signed with a bare public key instead of a
    /// Fulcio certificate, and when the certificate could not be parsed, in
    /// which case [`Self::warnings`] explains why.
    pub certificate: Option<SigningCertificate>,
    /// The transparency log entry that records the signature, which locates it
    /// in a public log for independent auditing.
    pub log_entry: Option<TransparencyLogEntry>,
    /// Which parts of the Sigstore verification were performed.
    pub checks: VerifiedChecks,
    /// Non-fatal observations made during verification.
    pub warnings: Vec<String>,
}

impl VerifiedAttestation {
    /// The CI claims of the signing certificate.
    pub fn claims(&self) -> Option<&CertificateClaims> {
        Some(&self.certificate.as_ref()?.claims)
    }

    /// The index of the transparency log entry, which identifies the signature
    /// within its log.
    pub fn log_index(&self) -> Option<u64> {
        Some(self.log_entry.as_ref()?.log_index.value())
    }

    /// The origin of the transparency log the signature was recorded in, e.g.
    /// `rekor.sigstore.dev`.
    ///
    /// This is taken from the signed checkpoint, so it is only available for a
    /// bundle that carries an inclusion proof.
    pub fn log_origin(&self) -> Option<&str> {
        let entry = self.log_entry.as_ref()?;
        let checkpoint = entry.inclusion_proof.as_ref()?.checkpoint.checkpoint()?;
        Some(checkpoint.origin.as_str())
    }
}

/// Which parts of the Sigstore verification of a bundle were performed.
///
/// A claim about a signature is only as strong as the checks behind it, so
/// these flags let a consumer report what was actually established rather than
/// implying a full verification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct VerifiedChecks {
    /// The certificate chains to a trusted Fulcio root, has the code signing
    /// extended key usage and was valid at every verified signing time.
    pub certificate_chain: bool,
    /// The certificate's signed certificate timestamp was verified against the
    /// certificate transparency log keys.
    pub signed_certificate_timestamp: bool,
    /// Inclusion in the transparency log was verified.
    pub transparency_log: bool,
    /// The bundle carried a full inclusion proof rather than only an inclusion
    /// promise, so log membership was checked against a signed checkpoint
    /// instead of being taken on the log's word.
    pub inclusion_proof: bool,
}

impl VerifiedChecks {
    fn new(result: &VerificationResult, bundle: &Bundle) -> Self {
        Self {
            certificate_chain: result.certificate_verified(),
            signed_certificate_timestamp: result.sct_verified(),
            transparency_log: result.tlog_verified(),
            inclusion_proof: bundle.has_inclusion_proof(),
        }
    }
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

/// Returns the trusted root snapshot that is embedded in the binary.
///
/// [`production_trusted_root`] is the root to use whenever the network can be
/// reached: it is fetched through TUF and therefore reflects key rotations and
/// revocations. The embedded snapshot is a copy of the public good instance's
/// root taken when `sigstore-trust-root` was released, so it ages with this
/// crate's dependencies and is only a sensible choice when TUF is unavailable,
/// such as for an offline verification.
pub fn embedded_trusted_root() -> SigstoreResult<TrustedRoot> {
    TrustedRoot::from_embedded(SigstoreInstance::PublicGood)
        .map_err(|err| SigstoreError::TrustedRoot(err.to_string()))
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
    let artifact = Artifact::from_digest(
        sigstore_types::Sha256Hash::try_from_slice(sha256.as_slice())
            .expect("rattler SHA-256 digests always contain 32 bytes"),
    );
    let expected_channel = expected_channel(record);

    let verifier =
        Verifier::new(trusted_root).map_err(|err| SigstoreError::TrustedRoot(err.to_string()))?;
    // Publisher policy is applied separately to the verified identities.
    let sigstore_policy = SigstoreVerificationPolicy::any_identity();

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
    let target_channel = validate_statement(&statement, filename)?;

    let mut warnings = Vec::new();
    if let (Some(target_channel), Some(expected)) = (target_channel.as_deref(), expected_channel) {
        let target = Url::parse(target_channel)
            .expect("validate_statement guarantees that targetChannel is a valid URL");
        if normalize_channel_url(target.clone()) != *expected {
            let message = format!(
                "the attestation targets channel {:?} but the package was retrieved from {}",
                target.redact(),
                expected.clone().redact()
            );
            match channel_check {
                ChannelCheck::Require => return Err(message),
                ChannelCheck::Warn => warnings.push(message),
                ChannelCheck::Ignore => {}
            }
        }
    }

    // The certificate was already parsed and verified by `sigstore-verify`, so
    // a failure here only means that the CI claims are unavailable and must not
    // invalidate an otherwise good signature.
    let certificate = match bundle.signing_certificate() {
        Some(der) => match SigningCertificate::from_der(der.as_bytes()) {
            Ok(certificate) => Some(certificate),
            Err(err) => {
                warnings.push(format!(
                    "the signing certificate claims could not be read: {err}"
                ));
                None
            }
        },
        None => None,
    };

    Ok(VerifiedAttestation {
        index: 0,
        identity: outcome.identity().map(str::to_owned),
        issuer: outcome.issuer().map(str::to_owned),
        integrated_time: outcome.integrated_time(),
        target_channel,
        certificate,
        log_entry: bundle.verification_material.tlog_entries.first().cloned(),
        checks: VerifiedChecks::new(&outcome, bundle),
        warnings,
    })
}

fn validate_statement(
    statement: &CondaPublishStatement,
    filename: &str,
) -> Result<Option<String>, String> {
    if statement.type_ != IN_TOTO_STATEMENT_TYPE {
        return Err(format!(
            "unexpected statement type {:?}, expected {IN_TOTO_STATEMENT_TYPE:?}",
            statement.type_
        ));
    }
    if statement.predicate_type != CONDA_PUBLISH_PREDICATE_TYPE {
        return Err(format!(
            "unexpected predicate type {:?}, expected {CONDA_PUBLISH_PREDICATE_TYPE:?}",
            statement.predicate_type
        ));
    }
    let [subject] = statement.subject.as_slice() else {
        return Err(format!(
            "the conda publish statement must have exactly one subject, found {}",
            statement.subject.len()
        ));
    };
    if subject.name != filename {
        return Err(format!(
            "the attestation subject is {:?}, expected {filename:?}",
            subject.name
        ));
    }

    let Some(predicate) = statement.predicate.as_ref() else {
        return Ok(None);
    };
    if predicate.target_channel.chars().count() > 2083 {
        return Err(
            "the conda publish predicate targetChannel exceeds 2083 characters".to_string(),
        );
    }
    if predicate.target_channel.ends_with('/') {
        return Err(
            "the conda publish predicate targetChannel must not have a trailing slash".to_string(),
        );
    }
    Url::parse(&predicate.target_channel).map_err(|err| {
        format!(
            "the conda publish predicate targetChannel {:?} is not a valid URL: {err}",
            predicate.target_channel
        )
    })?;
    Ok(Some(predicate.target_channel.clone()))
}

/// Extracts the in-toto statement from a DSSE bundle.
fn statement_of(bundle: &Bundle) -> Result<CondaPublishStatement, String> {
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
/// [`VerificationPolicy::Warn`] mode problems, including failure to load the
/// trusted root, are returned as warnings.
pub async fn verify_record(
    policy: &VerificationPolicy,
    record: &RepoDataRecord,
    client: &ClientWithMiddleware,
) -> SigstoreResult<VerificationOutcome> {
    if !policy.is_enabled() {
        return Ok(VerificationOutcome::default());
    }
    if let Some(err) = record_metadata_error(record) {
        return lenient(policy, err);
    }
    let trusted_root = match production_trusted_root().await {
        Ok(root) => root,
        Err(err) => return lenient(policy, err),
    };
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
    if let Some(err) = record_metadata_error(record) {
        return lenient(policy, err);
    }

    let sidecar = match fetch_sidecar(client, record, config.max_sidecar_size()).await {
        Ok(Some(sidecar)) => sidecar,
        // The metadata check above established that the record advertises a
        // sidecar.
        Ok(None) => unreachable!("record metadata changed during verification"),
        Err(err) => return lenient(policy, err),
    };

    let outcome = apply_policy(
        record,
        &sidecar,
        config.publisher(),
        config.channel_check(),
        trusted_root,
    );
    match outcome {
        Ok(outcome) => Ok(outcome),
        Err(err) => lenient(policy, err),
    }
}

/// Returns an error for record metadata that makes attestation verification
/// impossible, without loading the trusted root or fetching a sidecar.
fn record_metadata_error(record: &RepoDataRecord) -> Option<SigstoreError> {
    let filename = record.identifier.to_file_name();
    if record.package_record.attestations_sha256.is_none() {
        return Some(SigstoreError::NoAttestationsAdvertised(filename));
    }
    if record.package_record.sha256.is_none() {
        return Some(SigstoreError::MissingPackageSha256(filename));
    }
    None
}

/// Verifies the bundles of `sidecar` and picks the first one matching `publisher`.
fn apply_policy(
    record: &RepoDataRecord,
    sidecar: &AttestationSidecar,
    publisher: &Publisher,
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
        let accepted = publisher.matches(
            attestation.identity.as_deref(),
            attestation.issuer.as_deref(),
        );
        if accepted {
            tracing::debug!(
                "attestation {} of {} verified (identity: {:?}, issuer: {:?})",
                attestation.index,
                sidecar.url.clone().redact(),
                attestation.identity,
                attestation.issuer
            );
            return Ok(VerificationOutcome {
                warnings: attestation.warnings.clone(),
                attestation: Some(attestation),
            });
        }
        reasons.push(format!(
            "bundle {}: valid signature by {} (issuer {}) does not match the trusted publisher",
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::VerificationConfig;

    use super::*;

    const FILENAME: &str = "demo-1.0-0.conda";
    const SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn statement_json(predicate: Option<Value>) -> Value {
        let mut statement = json!({
            "_type": IN_TOTO_STATEMENT_TYPE,
            "subject": [{
                "name": FILENAME,
                "digest": {"sha256": SHA256},
            }],
            "predicateType": CONDA_PUBLISH_PREDICATE_TYPE,
        });
        if let Some(predicate) = predicate {
            statement["predicate"] = predicate;
        }
        statement
    }

    fn parse_statement(value: Value) -> CondaPublishStatement {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn predicate_may_be_omitted_or_null() {
        for predicate in [None, Some(Value::Null)] {
            let statement = parse_statement(statement_json(predicate));
            assert_eq!(validate_statement(&statement, FILENAME).unwrap(), None);
        }
    }

    #[test]
    fn object_predicate_requires_a_valid_target_channel() {
        assert!(
            serde_json::from_value::<CondaPublishStatement>(statement_json(Some(json!({}))))
                .is_err()
        );

        for target_channel in ["not a URL", "https://prefix.dev/conda-forge/"] {
            let statement = parse_statement(statement_json(Some(json!({
                "targetChannel": target_channel,
            }))));
            assert!(validate_statement(&statement, FILENAME).is_err());
        }

        let statement = parse_statement(statement_json(Some(json!({
            "targetChannel": "https://prefix.dev/conda-forge",
        }))));
        assert_eq!(
            validate_statement(&statement, FILENAME).unwrap().as_deref(),
            Some("https://prefix.dev/conda-forge")
        );
    }

    #[test]
    fn conda_publish_statement_requires_exactly_one_subject() {
        for subjects in [
            json!([]),
            json!([
                {"name": FILENAME, "digest": {"sha256": SHA256}},
                {"name": "other.conda", "digest": {"sha256": SHA256}},
            ]),
        ] {
            let mut value = statement_json(Some(Value::Null));
            value["subject"] = subjects;
            let statement = parse_statement(value);
            assert!(validate_statement(&statement, FILENAME).is_err());
        }
    }

    #[test]
    fn trusted_root_failure_obeys_verification_policy() {
        let warning = lenient(
            &VerificationPolicy::Warn(VerificationConfig::new(Publisher::new())),
            SigstoreError::TrustedRoot("offline".to_string()),
        )
        .unwrap();
        assert_eq!(warning.warnings.len(), 1);

        let required = lenient(
            &VerificationPolicy::Require(VerificationConfig::new(Publisher::new())),
            SigstoreError::TrustedRoot("offline".to_string()),
        );
        assert!(matches!(required, Err(SigstoreError::TrustedRoot(_))));
    }
}
