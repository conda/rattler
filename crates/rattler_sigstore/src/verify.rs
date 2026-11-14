//! Verification of attestation bundles against a package record.
//!
//! Verification has two layers:
//!
//! 1. Standard Sigstore verification of each bundle (signature, certificate
//!    chain, signed certificate timestamp, transparency log inclusion and the
//!    binding between the in-toto subject digest and the package SHA256),
//!    performed by `sigstore-verify` against the package record's `sha256`.
//!    The package archive itself is never needed.
//! 2. The [CEP 27](https://conda.org/learn/ceps/cep-0027) checks on the in-toto
//!    statement: the predicate type must be the conda publish predicate, the
//!    subject name must equal the package filename and, depending on the
//!    [`ChannelCheck`], the `targetChannel` must match the channel the package
//!    was retrieved from.
//!
//! [CEP 50](https://conda.org/learn/ceps/cep-0050) governs how the sidecar is
//! distributed but adds no publish-attestation verification rules, so only the
//! CEP 27 rules apply here.
//!
//! Which signing identities are trusted is decided afterwards by the
//! [`VerificationPolicy`].
//!
//! Bundles are normally signed with the Sigstore public-good instance and
//! recorded in its transparency log. GitHub signs attestations of private and
//! internal repositories with its own instance instead, which keeps no log:
//! such a bundle carries an RFC 3161 timestamp from GitHub's timestamp
//! authority and is verified against the trusted root of a GitHub trust
//! domain: github.com, or a GitHub Enterprise Cloud data residency region (see
//! [`TrustedRoots`] and [`VerificationConfig::with_github_trust_domains`]).
//! github.com is trusted by default, regions only when they are configured.

use jiff::Timestamp;
use rattler_conda_types::RepoDataRecord;
use rattler_redaction::Redact;
use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;
use sigstore_types::{Artifact, Bundle, SignatureContent, TransparencyLogEntry, intoto::Subject};
use sigstore_verify::{
    VerificationPolicy as SigstoreVerificationPolicy, VerificationResult, Verifier,
    crypto::{CertificateInfo, FulcioCiClaims},
    trust_root::{SigstoreInstance, TrustedRoot, TufConfig},
};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::OnceCell;
use url::Url;

use crate::{
    error::{SigstoreError, SigstoreResult},
    policy::{
        ChannelCheck, GitHubActionsIssuer, GitHubTrustDomain, Publisher, VerificationConfig,
        VerificationPolicy, normalize_channel_url,
    },
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
    /// The signing certificate the signature was verified against: its validity
    /// window and the claims it makes about the CI workload that signed the
    /// package.
    ///
    /// Fulcio issues short-lived certificates, so
    /// [`not_before`](CertificateInfo::not_before) is within seconds of the
    /// moment the package was signed and is the best available answer to "when
    /// was this signed?".
    ///
    /// This is `None` for a bundle signed with a bare public key instead of a
    /// Fulcio certificate.
    pub certificate: Option<CertificateInfo>,
    /// The transparency log entry that records the signature, which locates it
    /// in a public log for independent auditing.
    pub log_entry: Option<TransparencyLogEntry>,
    /// Which parts of the Sigstore verification were performed.
    pub checks: VerifiedChecks,
    /// The Sigstore instance whose trusted root the bundle verified against:
    /// [`SigstoreInstance::PublicGood`], or [`SigstoreInstance::GitHub`] for
    /// an attestation of a private or internal GitHub repository, which is not
    /// recorded in a public transparency log.
    pub instance: SigstoreInstance,
    /// Non-fatal observations made during verification.
    pub warnings: Vec<String>,
}

impl VerifiedAttestation {
    /// The CI claims of the signing certificate.
    pub fn claims(&self) -> Option<&FulcioCiClaims> {
        Some(&self.certificate.as_ref()?.ci_claims)
    }

    /// The index of the transparency log entry, which identifies the signature
    /// within its log.
    pub fn log_index(&self) -> Option<u64> {
        Some(self.log_entry.as_ref()?.log_index.get())
    }

    /// The origin of the transparency log the signature was recorded in, e.g.
    /// `rekor.sigstore.dev`.
    ///
    /// This is taken from the signed checkpoint, so it is only available for a
    /// bundle that carries an inclusion proof.
    pub fn log_origin(&self) -> Option<&str> {
        let entry = self.log_entry.as_ref()?;
        let checkpoint = entry.inclusion_proof.as_ref()?.checkpoint.checkpoint()?;
        Some(checkpoint.origin())
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

/// The trusted roots bundles are verified against.
///
/// Bundles with a transparency log entry are verified against the public-good
/// root. Bundles without one can only come from a GitHub artifact attestation
/// instance, which GitHub uses for private and internal repositories; they are
/// verified against the GitHub roots that were added, of which one must
/// accept the bundle, and rejected when there are none. A GitHub root that
/// lists a transparency log of its own is also tried for bundles with a log
/// entry that the public-good root does not accept.
///
/// Each GitHub root belongs to a [`GitHubTrustDomain`] and only vouches for
/// the GitHub Actions identities of that domain (see
/// [`GitHubTrustDomain::issues_for`]): github.com's root for github.com
/// workflows and a region's root for GitHub Enterprise Cloud tenants. Without
/// a public log nothing else would reveal a certificate one instance issued
/// for another's identities.
///
/// Like `gh attestation verify`, add the root of every GitHub trust domain
/// whose attestations should be accepted: github.com and, for GitHub
/// Enterprise Cloud tenants with data residency, the region of the tenant (see
/// [`github_trusted_root`]).
#[derive(Debug, Clone)]
pub struct TrustedRoots<'a> {
    public_good: &'a TrustedRoot,
    github: Vec<(GitHubTrustDomain, &'a TrustedRoot)>,
    /// Configured GitHub trust domains whose root could not be loaded, with
    /// the reason. They are named in the rejection of the bundles that would
    /// have needed them.
    github_unavailable: Vec<(GitHubTrustDomain, String)>,
}

impl<'a> TrustedRoots<'a> {
    /// Roots that only trust the public-good instance.
    pub fn new(public_good: &'a TrustedRoot) -> Self {
        Self {
            public_good,
            github: Vec::new(),
            github_unavailable: Vec::new(),
        }
    }

    /// Also trust the GitHub artifact attestation instance of `trust_domain`,
    /// whose bundles carry no transparency log entry. Can be called once per
    /// trust domain.
    pub fn with_github(mut self, trust_domain: GitHubTrustDomain, root: &'a TrustedRoot) -> Self {
        self.github.push((trust_domain, root));
        self
    }

    /// Records that the root of the trusted GitHub trust domain `trust_domain`
    /// could not be loaded, for example because GitHub's TUF repository could
    /// not be reached. Nothing is verified against it, but the rejection of a
    /// bundle that would have needed it names `reason` instead of only saying
    /// that GitHub's instance is not trusted.
    pub fn with_unavailable_github(
        mut self,
        trust_domain: GitHubTrustDomain,
        reason: impl Into<String>,
    ) -> Self {
        self.github_unavailable.push((trust_domain, reason.into()));
        self
    }

    /// The root bundles with a transparency log entry are verified against.
    pub fn public_good(&self) -> &'a TrustedRoot {
        self.public_good
    }

    /// The roots of the trusted GitHub artifact attestation instances.
    pub fn github(&self) -> &[(GitHubTrustDomain, &'a TrustedRoot)] {
        &self.github
    }
}

impl<'a> From<&'a TrustedRoot> for TrustedRoots<'a> {
    fn from(public_good: &'a TrustedRoot) -> Self {
        Self::new(public_good)
    }
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

/// How long a fetched GitHub trusted root is used before it is fetched again.
/// GitHub rotates its certificate and timestamp authorities, so a long-running
/// process must not keep a root forever.
const GITHUB_TRUSTED_ROOT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a failed fetch of a GitHub trusted root is remembered, so that
/// verifying many records while GitHub's TUF repository is unreachable does
/// not repeat the fetch for every record.
const GITHUB_TRUSTED_ROOT_RETRY_AFTER: Duration = Duration::from_secs(60);

/// The cached trusted root of one GitHub trust domain.
#[derive(Default)]
struct CachedGitHubRoot {
    /// The last root that was fetched, and when.
    root: Option<(Instant, Arc<TrustedRoot>)>,
    /// The last failed fetch, and when.
    failure: Option<(Instant, String)>,
}

/// Process-wide cache of the trusted roots of GitHub trust domains, one entry
/// per trust domain that was asked for. The async mutex of an entry serializes
/// the fetches of that trust domain.
static GITHUB_TRUSTED_ROOTS: LazyLock<
    Mutex<HashMap<GitHubTrustDomain, Arc<tokio::sync::Mutex<CachedGitHubRoot>>>>,
> = LazyLock::new(Default::default);

/// Returns the trusted root of a GitHub artifact attestation trust domain,
/// fetching it through GitHub's TUF repository.
///
/// GitHub signs attestations of private and internal repositories with these
/// instances: github.com with [`GitHubTrustDomain::github_com`], GitHub
/// Enterprise Cloud tenants with data residency with the instance of their
/// region.
///
/// The root is cached per process and trust domain and fetched again once a
/// day, so that a long-running process follows GitHub's key rotations. If
/// that refresh fails, the previous root keeps being used. A failed fetch is
/// remembered for a minute before it is tried again.
pub async fn github_trusted_root(
    trust_domain: &GitHubTrustDomain,
) -> SigstoreResult<Arc<TrustedRoot>> {
    let entry = {
        let mut entries = GITHUB_TRUSTED_ROOTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(entries.entry(trust_domain.clone()).or_default())
    };
    let mut cached = entry.lock().await;
    refresh_github_root(&mut cached, Instant::now, trust_domain, || async {
        TrustedRoot::from_tuf_target(TufConfig::github(), &trust_domain.trusted_root_target())
            .await
            .map_err(|err| err.to_string())
    })
    .await
}

/// Returns the cached root if it is fresh, and otherwise fetches it with
/// `fetch`, unless a fetch failed recently.
///
/// `clock` is read again once the fetch has finished: a fetch that fails
/// slowly, such as one that runs into a timeout, must still be remembered for
/// the whole retry interval, or everyone waiting for it would repeat it.
async fn refresh_github_root<C, F, Fut>(
    cached: &mut CachedGitHubRoot,
    clock: C,
    trust_domain: &GitHubTrustDomain,
    fetch: F,
) -> SigstoreResult<Arc<TrustedRoot>>
where
    C: Fn() -> Instant,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<TrustedRoot, String>>,
{
    let now = clock();
    let stale = match &cached.root {
        Some((fetched, root)) if now.duration_since(*fetched) < GITHUB_TRUSTED_ROOT_TTL => {
            return Ok(Arc::clone(root));
        }
        Some((_, root)) => Some(Arc::clone(root)),
        None => None,
    };

    let failure = match &cached.failure {
        Some((failed, message))
            if now.duration_since(*failed) < GITHUB_TRUSTED_ROOT_RETRY_AFTER =>
        {
            message.clone()
        }
        _ => match fetch().await {
            Ok(root) => {
                let root = Arc::new(root);
                cached.root = Some((clock(), Arc::clone(&root)));
                cached.failure = None;
                return Ok(root);
            }
            Err(err) => {
                let message = format!("GitHub trust domain {trust_domain}: {err}");
                cached.failure = Some((clock(), message.clone()));
                message
            }
        },
    };

    match stale {
        Some(root) => {
            tracing::warn!(
                "failed to refresh the Sigstore trusted root, using the previous one: {failure}"
            );
            Ok(root)
        }
        None => Err(SigstoreError::TrustedRoot(failure)),
    }
}

/// Returns the trusted root of a GitHub trust domain that is embedded in the
/// binary, for verification without network access.
///
/// Like [`embedded_trusted_root`] this is a snapshot that ages with this
/// crate's dependencies. Only github.com's root is embedded; the roots of
/// GitHub Enterprise Cloud data residency regions have to be fetched with
/// [`github_trusted_root`].
pub fn embedded_github_trusted_root(
    trust_domain: &GitHubTrustDomain,
) -> SigstoreResult<TrustedRoot> {
    if !trust_domain.is_github_com() {
        return Err(SigstoreError::TrustedRoot(format!(
            "GitHub trust domain {trust_domain}: only the trusted root of github.com is embedded"
        )));
    }
    TrustedRoot::from_embedded(SigstoreInstance::GitHub)
        .map_err(|err| SigstoreError::TrustedRoot(err.to_string()))
}

/// Returns true if `bundle` carries no transparency log entry, which is the
/// case for bundles signed by GitHub's artifact attestation instance.
fn lacks_transparency_log_entry(bundle: &Bundle) -> bool {
    bundle.verification_material.tlog_entries.is_empty()
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
///
/// Only `trusted_root` is trusted, so bundles without a transparency log
/// entry (attestations of private GitHub repositories) are rejected. Use
/// [`verify_bundles_with_trusted_roots`] to also trust GitHub's instance.
pub fn verify_bundles(
    record: &RepoDataRecord,
    bundles: &[Bundle],
    channel_check: ChannelCheck,
    trusted_root: &TrustedRoot,
) -> SigstoreResult<BundleVerification> {
    verify_bundles_with_trusted_roots(
        record,
        bundles,
        channel_check,
        TrustedRoots::new(trusted_root),
    )
}

/// Like [`verify_bundles`], but with the roots of several Sigstore instances.
///
/// Bundles without a transparency log entry are only accepted when
/// `trusted_roots` includes a GitHub root that verifies them, and only for
/// GitHub Actions identities. See [`TrustedRoots`].
pub fn verify_bundles_with_trusted_roots(
    record: &RepoDataRecord,
    bundles: &[Bundle],
    channel_check: ChannelCheck,
    trusted_roots: TrustedRoots<'_>,
) -> SigstoreResult<BundleVerification> {
    let filename = record.identifier.to_file_name();
    let sha256 = record
        .package_record
        .sha256
        .as_ref()
        .ok_or_else(|| SigstoreError::MissingPackageSha256(filename.clone()))?;
    let artifact = Artifact::from_digest(
        sigstore_types::Sha256Hash::try_from(sha256.as_slice())
            .expect("rattler SHA-256 digests always contain 32 bytes"),
    );
    let expected_channel = expected_channel(record);

    let public_good = InstanceVerifier::public_good(trusted_roots.public_good)?;
    // Only build the GitHub verifiers when a bundle can need them: a bundle
    // without a log entry, or a root that keeps a log of its own. A root that
    // cannot be used only affects the bundles that would have needed it.
    let github: Vec<GitHubInstance> = if bundles.iter().any(lacks_transparency_log_entry)
        || trusted_roots
            .github
            .iter()
            .any(|(_, root)| !root.tlogs.is_empty())
    {
        trusted_roots
            .github
            .iter()
            .map(|(trust_domain, root)| GitHubInstance::new(trust_domain, root))
            .chain(
                trusted_roots
                    .github_unavailable
                    .iter()
                    .map(|(trust_domain, reason)| {
                        GitHubInstance::unavailable(trust_domain, reason)
                    }),
            )
            .collect()
    } else {
        Vec::new()
    };

    let mut result = BundleVerification::default();
    for (index, bundle) in bundles.iter().enumerate() {
        // A bundle names no trusted root, so try each candidate, as
        // `gh attestation verify` does with several trusted roots.
        let lacks_log_entry = lacks_transparency_log_entry(bundle);
        let candidates: Vec<Candidate<'_>> = (!lacks_log_entry)
            .then_some(Candidate::Ready(&public_good))
            .into_iter()
            .chain(
                github
                    .iter()
                    .filter(|instance| instance.is_candidate_for(lacks_log_entry))
                    .map(GitHubInstance::candidate),
            )
            .collect();
        if candidates.is_empty() {
            result.rejected.push(RejectedAttestation {
                index,
                reason: "the bundle has no transparency log entry, as signed by \
                         GitHub's artifact attestation instance for private \
                         repositories, which is not trusted"
                    .to_string(),
            });
            continue;
        }

        let mut reasons = Vec::new();
        let verified = candidates.iter().find_map(|candidate| {
            let outcome = match candidate {
                Candidate::Ready(instance) => verify_bundle(
                    instance,
                    artifact.clone(),
                    bundle,
                    &filename,
                    expected_channel.as_ref(),
                    channel_check,
                ),
                Candidate::Unavailable { reason, .. } => {
                    Err(format!("the trusted root could not be loaded: {reason}"))
                }
            };
            match outcome {
                Ok(verified) => Some(verified),
                Err(reason) => {
                    reasons.push(if candidates.len() > 1 {
                        format!("{}: {reason}", candidate.name())
                    } else {
                        reason
                    });
                    None
                }
            }
        });
        match verified {
            Some(mut verified) => {
                verified.index = index;
                result.verified.push(verified);
            }
            None => result.rejected.push(RejectedAttestation {
                index,
                reason: reasons.join("; "),
            }),
        }
    }
    Ok(result)
}

/// A trusted GitHub instance: its verifier, or why it cannot be used.
struct GitHubInstance {
    name: String,
    verifier: Result<InstanceVerifier, String>,
}

impl GitHubInstance {
    fn new(trust_domain: &GitHubTrustDomain, trusted_root: &TrustedRoot) -> Self {
        Self {
            name: github_instance_name(trust_domain),
            verifier: InstanceVerifier::github(trust_domain, trusted_root)
                .map_err(|err| err.to_string()),
        }
    }

    fn unavailable(trust_domain: &GitHubTrustDomain, reason: &str) -> Self {
        Self {
            name: github_instance_name(trust_domain),
            verifier: Err(reason.to_string()),
        }
    }

    /// Whether this instance can have signed a bundle with or without a
    /// transparency log entry. An instance without a log signs bundles without
    /// an entry; one that keeps a log signs bundles with an entry, although a
    /// bundle without one is still passed to it so that the rejection names
    /// the missing entry. Nothing is known about an instance whose root could
    /// not be loaded, so it is only reported for bundles without an entry,
    /// which no other root can accept.
    fn is_candidate_for(&self, lacks_log_entry: bool) -> bool {
        match &self.verifier {
            Ok(verifier) => lacks_log_entry || verifier.requires_log_entry,
            Err(_) => lacks_log_entry,
        }
    }

    fn candidate(&self) -> Candidate<'_> {
        match &self.verifier {
            Ok(verifier) => Candidate::Ready(verifier),
            Err(reason) => Candidate::Unavailable {
                name: &self.name,
                reason,
            },
        }
    }
}

fn github_instance_name(trust_domain: &GitHubTrustDomain) -> String {
    format!("GitHub instance of {trust_domain}")
}

/// An instance a bundle is verified against.
enum Candidate<'a> {
    Ready(&'a InstanceVerifier),
    /// A trusted instance whose root could not be loaded.
    Unavailable {
        name: &'a str,
        reason: &'a str,
    },
}

impl Candidate<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Ready(instance) => &instance.name,
            Self::Unavailable { name, .. } => name,
        }
    }
}

/// A verifier for the bundles of one Sigstore instance.
struct InstanceVerifier {
    instance: SigstoreInstance,
    /// Names the instance in rejection reasons.
    name: String,
    /// The trust domain of a GitHub instance.
    github_trust_domain: Option<GitHubTrustDomain>,
    /// Whether the instance keeps a transparency log, so that its bundles
    /// carry a log entry.
    requires_log_entry: bool,
    verifier: Verifier,
    policy: SigstoreVerificationPolicy,
}

impl InstanceVerifier {
    fn public_good(trusted_root: &TrustedRoot) -> SigstoreResult<Self> {
        Ok(Self {
            instance: SigstoreInstance::PublicGood,
            name: "public-good instance".to_string(),
            github_trust_domain: None,
            requires_log_entry: true,
            verifier: Verifier::new(trusted_root)
                .map_err(|err| SigstoreError::TrustedRoot(err.to_string()))?,
            // Publisher policy is applied separately to the verified identities.
            policy: SigstoreVerificationPolicy::any_identity(),
        })
    }

    /// The policy follows what the trusted root advertises. GitHub's roots
    /// list neither a transparency log nor a certificate transparency log, so
    /// bundles carry no log entry and certificates no SCT; the signing time
    /// is established by the RFC 3161 timestamp instead, which the verifier
    /// requires when there is no log entry. Should a root ever list a log,
    /// the corresponding check is enforced again, and the instance is also
    /// tried for bundles that carry a log entry.
    fn github(
        trust_domain: &GitHubTrustDomain,
        trusted_root: &TrustedRoot,
    ) -> SigstoreResult<Self> {
        let mut policy = SigstoreVerificationPolicy::any_identity();
        if trusted_root.tlogs.is_empty() {
            policy = policy.skip_tlog_unsafe();
        }
        if trusted_root.ctlogs.is_empty() {
            policy = policy.skip_sct();
        }
        Ok(Self {
            instance: SigstoreInstance::GitHub,
            name: github_instance_name(trust_domain),
            github_trust_domain: Some(trust_domain.clone()),
            requires_log_entry: !trusted_root.tlogs.is_empty(),
            verifier: Verifier::new(trusted_root)
                .map_err(|err| SigstoreError::TrustedRoot(err.to_string()))?,
            policy,
        })
    }
}

fn verify_bundle(
    instance: &InstanceVerifier,
    artifact: Artifact<'_>,
    bundle: &Bundle,
    filename: &str,
    expected_channel: Option<&Url>,
    channel_check: ChannelCheck,
) -> Result<VerifiedAttestation, String> {
    let outcome = instance
        .verifier
        .verify(artifact, bundle, &instance.policy)
        .map_err(|err| format!("sigstore verification failed: {err}"))?;

    // Without a public log, nothing would reveal a certificate a GitHub
    // instance issued for an identity it does not own, so only accept the
    // GitHub Actions identities of the instance's own trust domain.
    if let Some(trust_domain) = &instance.github_trust_domain {
        let issued_for_domain = outcome
            .issuer()
            .and_then(GitHubActionsIssuer::parse)
            .is_some_and(|issuer| trust_domain.issues_for(&issuer));
        if !issued_for_domain {
            return Err(format!(
                "the bundle was signed by the GitHub artifact attestation instance of {trust_domain} \
                 for issuer {}, but that instance is only trusted for GitHub Actions identities \
                 of {}",
                outcome.issuer().unwrap_or("<unknown issuer>"),
                if trust_domain.is_github_com() {
                    "github.com"
                } else {
                    "GitHub Enterprise Cloud tenants"
                }
            ));
        }
    }

    let statement = statement_of(bundle)?;
    let target_channel = validate_statement(&statement, filename)?;

    let mut warnings = Vec::new();
    if let (Some(target_channel), Some(expected)) = (target_channel.as_deref(), expected_channel) {
        // Compare and report the normalized form, so that the two URLs in the
        // message are the ones that were actually compared.
        let target = normalize_channel_url(
            Url::parse(target_channel)
                .expect("validate_statement guarantees that targetChannel is a valid URL"),
        );
        if target != *expected {
            let message = format!(
                "the attestation targets channel {} but the package was retrieved from {}",
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
        identity: outcome.identity().map(ToString::to_string),
        issuer: outcome.issuer().map(str::to_owned),
        integrated_time: outcome.integrated_time(),
        target_channel,
        certificate: outcome.certificate().cloned(),
        log_entry: bundle.verification_material.tlog_entries.first().cloned(),
        checks: VerifiedChecks::new(&outcome, bundle),
        instance: instance.instance,
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
    serde_json::from_slice(envelope.payload.as_bytes())
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
    verify_record_impl(
        policy,
        record,
        client,
        TrustedRoots::new(trusted_root),
        FetchGithubRoot::IfNeeded,
    )
    .await
}

/// Like [`verify_record`] but with an explicitly provided root of the
/// public-good instance, which is not fetched.
///
/// As with [`verify_record`], the roots of the GitHub trust domains of
/// [`VerificationConfig::github_trust_domains`] are fetched when a package
/// needs them, which by default is github.com's. Use
/// [`verify_record_with_trusted_roots`] to verify without fetching any root.
pub async fn verify_record_with_trusted_root(
    policy: &VerificationPolicy,
    record: &RepoDataRecord,
    client: &ClientWithMiddleware,
    trusted_root: &TrustedRoot,
) -> SigstoreResult<VerificationOutcome> {
    verify_record_impl(
        policy,
        record,
        client,
        TrustedRoots::new(trusted_root),
        FetchGithubRoot::IfNeeded,
    )
    .await
}

/// Like [`verify_record`] but with explicitly provided trusted roots, without
/// fetching any root over the network.
///
/// Bundles without a transparency log entry (attestations of private GitHub
/// repositories) are rejected unless GitHub roots are added with
/// [`TrustedRoots::with_github`]; [`VerificationConfig::github_trust_domains`]
/// is not consulted.
pub async fn verify_record_with_trusted_roots(
    policy: &VerificationPolicy,
    record: &RepoDataRecord,
    client: &ClientWithMiddleware,
    trusted_roots: TrustedRoots<'_>,
) -> SigstoreResult<VerificationOutcome> {
    verify_record_impl(
        policy,
        record,
        client,
        trusted_roots,
        FetchGithubRoot::Never,
    )
    .await
}

/// Whether [`verify_record_impl`] may fetch the roots of the configured GitHub
/// trust domains.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FetchGithubRoot {
    IfNeeded,
    Never,
}

async fn verify_record_impl(
    policy: &VerificationPolicy,
    record: &RepoDataRecord,
    client: &ClientWithMiddleware,
    trusted_roots: TrustedRoots<'_>,
    fetch_github_root: FetchGithubRoot,
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

    let mut outcome = apply_policy(record, &sidecar, config, trusted_roots.clone());

    // Only reach out to GitHub's TUF repository when the roots at hand do not
    // accept the package. A root that cannot be loaded only affects the
    // bundles that need it, whose rejection then names the failure.
    if outcome.is_err()
        && fetch_github_root == FetchGithubRoot::IfNeeded
        && !config.github_trust_domains().is_empty()
    {
        let mut fetched = Vec::new();
        let mut unavailable = Vec::new();
        for trust_domain in config.github_trust_domains() {
            match github_trusted_root(trust_domain).await {
                Ok(root) => fetched.push((trust_domain.clone(), root)),
                Err(err) => unavailable.push((trust_domain.clone(), err.to_string())),
            }
        }
        let mut with_github: TrustedRoots<'_> = trusted_roots;
        with_github.github.extend(
            fetched
                .iter()
                .map(|(trust_domain, root)| (trust_domain.clone(), root.as_ref())),
        );
        with_github.github_unavailable.extend(unavailable);
        outcome = apply_policy(record, &sidecar, config, with_github);
    }

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
    config: &VerificationConfig,
    trusted_roots: TrustedRoots<'_>,
) -> SigstoreResult<VerificationOutcome> {
    let publisher: &Publisher = config.publisher();
    let verification = verify_bundles_with_trusted_roots(
        record,
        &sidecar.bundles,
        config.channel_check(),
        trusted_roots,
    )?;

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

    fn github_root() -> TrustedRoot {
        TrustedRoot::from_embedded(SigstoreInstance::GitHub).unwrap()
    }

    async fn refresh(
        cached: &mut CachedGitHubRoot,
        now: Instant,
        fetches: &mut u32,
        result: Result<TrustedRoot, String>,
    ) -> SigstoreResult<Arc<TrustedRoot>> {
        refresh_github_root(
            cached,
            || now,
            &GitHubTrustDomain::github_com(),
            || {
                *fetches += 1;
                async move { result }
            },
        )
        .await
    }

    #[tokio::test]
    async fn a_slow_github_root_fetch_failure_is_remembered_from_when_it_failed() {
        let mut cached = CachedGitHubRoot::default();
        let start = Instant::now();
        // The fetch takes as long as the retry interval to fail.
        let finished = start + GITHUB_TRUSTED_ROOT_RETRY_AFTER;
        let now = std::cell::Cell::new(start);
        let result = refresh_github_root(
            &mut cached,
            || now.get(),
            &GitHubTrustDomain::github_com(),
            || async {
                now.set(finished);
                Err("timed out".to_string())
            },
        )
        .await;
        assert!(result.is_err());

        // Whoever waited for that fetch does not repeat it.
        let mut fetches = 0;
        let result = refresh(&mut cached, finished, &mut fetches, Ok(github_root())).await;
        assert!(result.is_err());
        assert_eq!(fetches, 0);
    }

    #[tokio::test]
    async fn github_root_is_cached_and_refreshed() {
        let mut cached = CachedGitHubRoot::default();
        let mut fetches = 0;
        let start = Instant::now();

        let first = refresh(&mut cached, start, &mut fetches, Ok(github_root()))
            .await
            .unwrap();
        assert_eq!(fetches, 1);

        // A fresh root is not fetched again.
        let later = start + GITHUB_TRUSTED_ROOT_TTL - Duration::from_secs(1);
        let again = refresh(&mut cached, later, &mut fetches, Err("unused".to_string()))
            .await
            .unwrap();
        assert_eq!(fetches, 1);
        assert!(Arc::ptr_eq(&first, &again));

        // Once it expires it is fetched again, so key rotations are picked up.
        let expired = start + GITHUB_TRUSTED_ROOT_TTL;
        let refreshed = refresh(&mut cached, expired, &mut fetches, Ok(github_root()))
            .await
            .unwrap();
        assert_eq!(fetches, 2);
        assert!(!Arc::ptr_eq(&first, &refreshed));

        // A failed refresh keeps serving the previous root, and is not
        // repeated right away.
        let expired = expired + GITHUB_TRUSTED_ROOT_TTL;
        for _ in 0..2 {
            let stale = refresh(
                &mut cached,
                expired,
                &mut fetches,
                Err("offline".to_string()),
            )
            .await
            .unwrap();
            assert!(Arc::ptr_eq(&refreshed, &stale));
        }
        assert_eq!(fetches, 3);
    }

    #[tokio::test]
    async fn github_root_fetch_failures_are_cached_briefly() {
        let mut cached = CachedGitHubRoot::default();
        let mut fetches = 0;
        let start = Instant::now();

        for _ in 0..3 {
            let err = refresh(&mut cached, start, &mut fetches, Err("offline".to_string()))
                .await
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                "failed to load the Sigstore trusted root: GitHub trust domain github.com: offline"
            );
        }
        assert_eq!(fetches, 1);

        // After the retry interval the fetch is attempted again.
        let later = start + GITHUB_TRUSTED_ROOT_RETRY_AFTER;
        refresh(&mut cached, later, &mut fetches, Ok(github_root()))
            .await
            .unwrap();
        assert_eq!(fetches, 2);
    }

    #[test]
    fn only_the_github_com_root_is_embedded() {
        assert!(embedded_github_trusted_root(&GitHubTrustDomain::github_com()).is_ok());
        let region = GitHubTrustDomain::region("prod-weu-01").unwrap();
        assert!(matches!(
            embedded_github_trusted_root(&region),
            Err(SigstoreError::TrustedRoot(_))
        ));
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
