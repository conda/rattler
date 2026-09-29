#![deny(missing_docs)]
//! Discovery, retrieval and verification of Sigstore attestations for conda
//! packages.
//!
//! This crate implements the client side of two conda CEPs:
//! [CEP 50](https://conda.org/learn/ceps/cep-0050), which standardizes how
//! Sigstore attestations are distributed alongside packages, and
//! [CEP 27](https://conda.org/learn/ceps/cep-0027), which standardizes the
//! attestation format and the rules for verifying one. CEP 50 adds no
//! publish-attestation verification rules of its own.
//!
//! - A package record advertises its attestations through the
//!   `attestations_sha256` field of the repodata (CEP 50).
//! - The attestations live in a sidecar at `<package_url>.sigs.<sha256>`, a
//!   JSON array of Sigstore bundles (CEP 50). See [`sidecar`].
//! - Each bundle is verified with `sigstore-verify` against the package's
//!   SHA256 and then checked against the CEP 27 rules for conda publish
//!   attestations. See [`verify`].
//! - A [`VerificationPolicy`] decides which signing identities are trusted for
//!   all packages and whether failures warn or block. Neither CEP prescribes
//!   this; CEP 50 leaves trust policy explicitly out of scope. See [`policy`].
//!
//! # Example
//!
//! ```no_run
//! use rattler_sigstore::{Issuer, Publisher, VerificationConfig, VerificationPolicy};
//!
//! # async fn example(record: rattler_conda_types::RepoDataRecord, client: reqwest_middleware::ClientWithMiddleware) -> Result<(), rattler_sigstore::SigstoreError> {
//! let config = VerificationConfig::new(
//!     Publisher::new()
//!         .with_identity("https://github.com/my-org/*")
//!         .with_issuer(Issuer::github_actions()),
//! );
//! let policy = VerificationPolicy::Require(config);
//!
//! let outcome = rattler_sigstore::verify_record(&policy, &record, &client).await?;
//! assert!(outcome.is_verified());
//! # Ok(())
//! # }
//! ```

pub mod error;
pub mod policy;
pub mod sidecar;
pub mod verify;

pub use error::{SigstoreError, SigstoreResult};
pub use policy::{
    ChannelCheck, Identity, Issuer, Publisher, VerificationConfig, VerificationPolicy,
};
pub use sidecar::{
    AttestationSidecar, DEFAULT_MAX_SIDECAR_SIZE, SIDECAR_SUFFIX, fetch_bundles, fetch_sidecar,
    mutable_sidecar_url, parse_bundles, parse_sidecar, sidecar_url, sidecar_url_for_record,
};
pub use sigstore_types::{Bundle, TransparencyLogEntry};
/// The signing certificate a verified attestation was produced with, including
/// the claims Fulcio copied from the workload identity of the signer: the
/// repository, the commit, the build configuration and the CI run. Both types
/// come from `sigstore-crypto` and are re-exported here so that a consumer of
/// this crate does not have to depend on it directly.
pub use sigstore_verify::crypto::{CertificateInfo, FulcioCiClaims};
pub use sigstore_verify::trust_root::TrustedRoot;
pub use verify::{
    BundleVerification, CONDA_PUBLISH_PREDICATE_TYPE, CondaPublishPredicate, RejectedAttestation,
    VerificationOutcome, VerifiedAttestation, VerifiedChecks, embedded_trusted_root,
    production_trusted_root, verify_bundles, verify_record, verify_record_with_trusted_root,
};
