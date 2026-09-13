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
use sigstore_types::{Artifact, Bundle, SignatureContent, intoto::Subject};
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
    let artifact = Artifact::from_digest(
        sigstore_types::Sha256Hash::try_from_slice(sha256.as_slice())
            .expect("rattler SHA-256 digests always contain 32 bytes"),
    );
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
    let target_channel = validate_statement(&statement, filename)?;

    let mut warnings = outcome.warnings;
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

    Ok(VerifiedAttestation {
        index: 0,
        identity: outcome.identity,
        issuer: outcome.issuer,
        integrated_time: outcome.integrated_time,
        target_channel,
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
    let filename = record.identifier.to_file_name();

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
        config.publisher(),
        config.channel_check(),
        trusted_root,
    );
    match outcome {
        Ok(outcome) => Ok(outcome),
        Err(err) => lenient(policy, err),
    }
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
