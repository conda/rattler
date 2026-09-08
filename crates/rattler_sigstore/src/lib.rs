#![deny(missing_docs)]
//! Discovery, retrieval and verification of Sigstore attestations for conda
//! packages.
//!
//! This crate implements the client side of the conda CEP on distribution of
//! Sigstore attestations together with the verification rules of CEP 27:
//!
//! - A package record advertises its attestations through the
//!   `attestations_sha256` field of the repodata.
//! - The attestations live in a sidecar at `<package_url>.sigs.<sha256>`, a
//!   JSON array of Sigstore bundles. See [`sidecar`].
//! - Each bundle is verified with `sigstore-verify` against the package's
//!   SHA256 and then checked against the CEP 27 rules for conda publish
//!   attestations. See [`verify`].
//! - A [`VerificationPolicy`] decides which signing identities are trusted for
//!   which channels and whether failures warn or block. See [`policy`].
//!
//! # Example
//!
//! ```no_run
//! use rattler_sigstore::{Issuer, Publisher, VerificationConfig, VerificationPolicy};
//! use url::Url;
//!
//! # async fn example(record: rattler_conda_types::RepoDataRecord, client: reqwest_middleware::ClientWithMiddleware) -> Result<(), rattler_sigstore::SigstoreError> {
//! let config = VerificationConfig::new().with_channel_publisher(
//!     Url::parse("https://prefix.dev/my-channel").unwrap(),
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
    AttestationSidecar, DEFAULT_MAX_SIDECAR_SIZE, SIDECAR_SUFFIX, fetch_sidecar, parse_sidecar,
    sidecar_url, sidecar_url_for_record,
};
pub use sigstore_types::Bundle;
pub use sigstore_verify::trust_root::TrustedRoot;
pub use verify::{
    BundleVerification, CONDA_PUBLISH_PREDICATE_TYPE, CondaPublishPredicate, RejectedAttestation,
    VerificationOutcome, VerifiedAttestation, production_trusted_root, verify_bundles,
    verify_record, verify_record_with_trusted_root,
};
