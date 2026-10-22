//! Command-line flags that configure Sigstore attestation verification for
//! commands that consume package records.

use clap::ArgGroup;
use futures_util::{StreamExt, stream};
use miette::IntoDiagnostic;
use rattler_conda_types::RepoDataRecord;
use rattler_sigstore::{ChannelCheck, VerificationConfig, VerificationPolicy};
use reqwest_middleware::ClientWithMiddleware;

use crate::publisher_args::PublisherArgs;

/// How strictly attestations are verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum VerifyMode {
    /// Every package needs an attestation that verifies against a trusted
    /// publisher; otherwise the command fails.
    Require,
    /// Verify attestations but only report problems.
    Warn,
}

/// Flatten this into a command's options with `#[clap(flatten)]`.
#[derive(Debug, Default, clap::Args)]
#[clap(group(
    ArgGroup::new("publisher_constraints")
        .args(["identity", "issuer"])
        .requires("verify_attestations")
        .multiple(true)
))]
pub struct AttestationPolicyArgs {
    /// Verify the Sigstore attestations of the selected packages.
    /// Attestations are discovered through the `attestations_sha256` field of
    /// the repodata. `require` fails the command when a package has no valid
    /// attestation, `warn` only reports problems.
    #[clap(long, value_name = "MODE", value_enum)]
    verify_attestations: Option<VerifyMode>,

    /// Signing certificate publisher constraints.
    #[clap(flatten)]
    publisher: PublisherArgs,

    /// Accept attestations whose `targetChannel` differs from the channel the
    /// package was retrieved from, e.g. when using a mirror.
    #[clap(long, requires = "verify_attestations")]
    allow_channel_mismatch: bool,
}

impl AttestationPolicyArgs {
    /// Builds the verification policy described by the flags.
    pub fn policy(&self) -> VerificationPolicy {
        let Some(mode) = self.verify_attestations else {
            return VerificationPolicy::Disabled;
        };

        let config = VerificationConfig::new(self.publisher.publisher()).with_channel_check(
            if self.allow_channel_mismatch {
                ChannelCheck::Warn
            } else {
                ChannelCheck::Require
            },
        );

        match mode {
            VerifyMode::Require => VerificationPolicy::Require(config),
            VerifyMode::Warn => VerificationPolicy::Warn(config),
        }
    }

    /// Verifies the attestations of the selected package records and writes
    /// verification information to stderr.
    pub async fn verify_records(
        &self,
        records: &[RepoDataRecord],
        client: &ClientWithMiddleware,
    ) -> miette::Result<()> {
        let policy = self.policy();
        if !policy.is_enabled() {
            return Ok(());
        }

        // Bound concurrency so a large solve does not issue every sidecar
        // request at once. Collect first to keep the diagnostics in the stable
        // package order chosen by the caller.
        let policy = &policy;
        let mut outcomes = stream::iter(records.iter().enumerate())
            .map(move |(index, record)| async move {
                (
                    index,
                    record,
                    rattler_sigstore::verify_record(policy, record, client).await,
                )
            })
            .buffer_unordered(16)
            .collect::<Vec<_>>()
            .await;
        outcomes.sort_unstable_by_key(|(index, _, _)| *index);

        let mut verified = 0;
        for (_, record, outcome) in outcomes {
            let outcome = outcome.into_diagnostic()?;
            for warning in outcome.warnings {
                eprintln!("warning: {}: {warning}", record.identifier);
            }
            if let Some(attestation) = outcome.attestation {
                verified += 1;
                eprintln!(
                    "Verified attestation for {} (identity: {}, issuer: {})",
                    record.identifier,
                    attestation.identity.as_deref().unwrap_or("unknown"),
                    attestation.issuer.as_deref().unwrap_or("unknown"),
                );
            }
        }
        eprintln!(
            "Verified attestations for {verified} of {} solved packages",
            records.len()
        );

        Ok(())
    }
}
