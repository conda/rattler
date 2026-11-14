//! Command-line flags that configure Sigstore attestation verification for
//! commands that consume package records.

use clap::ArgGroup;
use futures_util::{StreamExt, stream};
use miette::IntoDiagnostic;
use rattler_conda_types::RepoDataRecord;
use rattler_sigstore::{ChannelCheck, GitHubTrustDomain, VerificationConfig, VerificationPolicy};
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

    /// The GitHub trust domains whose Sigstore instances are trusted besides
    /// the Sigstore public-good instance: `github.com`, or a GitHub Enterprise
    /// Cloud data residency region such as `prod-weu-01`. Can be given more
    /// than once, which replaces the default.
    ///
    /// GitHub signs the attestations of private and internal repositories
    /// with these instances, which keep no public transparency log.
    #[clap(
        long,
        value_name = "DOMAIN",
        default_value = "github.com",
        requires = "verify_attestations"
    )]
    github_trust_domain: Vec<GitHubTrustDomain>,

    /// Only accept attestations that are recorded in the public transparency
    /// log of the Sigstore public-good instance.
    #[clap(
        long,
        requires = "verify_attestations",
        conflicts_with = "github_trust_domain"
    )]
    public_good_only: bool,
}

impl AttestationPolicyArgs {
    /// The GitHub trust domains to trust besides the public-good instance.
    fn github_trust_domains(&self) -> Vec<GitHubTrustDomain> {
        if self.public_good_only {
            Vec::new()
        } else {
            self.github_trust_domain.clone()
        }
    }

    /// Builds the verification policy described by the flags.
    pub fn policy(&self) -> VerificationPolicy {
        let Some(mode) = self.verify_attestations else {
            return VerificationPolicy::Disabled;
        };

        let config = VerificationConfig::new(self.publisher.publisher())
            .with_channel_check(if self.allow_channel_mismatch {
                ChannelCheck::Warn
            } else {
                ChannelCheck::Require
            })
            .with_github_trust_domains(self.github_trust_domains());

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
