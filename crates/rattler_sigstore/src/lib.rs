//! Sigstore signature verification for conda packages.
//!
//! This crate provides types and functions for verifying Sigstore signatures
//! on conda packages. It supports three verification modes:
//!
//! - **Disabled**: No signature verification
//! - **Warn**: Verify signatures but only warn on failure
//! - **Require**: Require signatures to be valid for package installation
//!
//! # Example
//!
//! ```no_run
//! use rattler_sigstore::{VerificationPolicy, VerificationConfig, Publisher, Issuer};
//! use url::Url;
//!
//! // Create a policy that requires verification for packages from conda-forge
//! let mut config = VerificationConfig::default();
//! config.add_channel_publisher(
//!     Url::parse("https://conda.anaconda.org/conda-forge/").unwrap(),
//!     Publisher::new()
//!         .with_issuer(Issuer::github_actions()),
//! );
//!
//! let policy = VerificationPolicy::Require(config);
//! ```

mod error;
mod policy;
mod verify;

pub use error::{SigstoreError, SigstoreResult};
pub use policy::{Identity, Issuer, Publisher, VerificationConfig, VerificationPolicy};
pub use verify::{verify_package, verify_package_by_digest, VerificationOutcome};
