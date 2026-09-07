//! Command-line flags that configure Sigstore attestation verification for
//! commands that install packages.

use rattler_sigstore::{ChannelCheck, Issuer, Publisher, VerificationConfig, VerificationPolicy};

/// How strictly attestations are verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum VerifyMode {
    /// Every package needs an attestation that verifies against a trusted
    /// publisher; otherwise the installation fails.
    Require,
    /// Verify attestations but only report problems.
    Warn,
}

/// Flatten this into a command's options with `#[clap(flatten)]`.
#[derive(Debug, Default, clap::Args)]
pub struct AttestationArgs {
    /// Verify the Sigstore attestations of the packages being installed.
    /// Attestations are discovered through the `attestations_sha256` field of
    /// the repodata. `require` fails the installation when a package has no
    /// valid attestation, `warn` only reports problems.
    #[clap(long, value_name = "MODE", value_enum)]
    verify_attestations: Option<VerifyMode>,

    /// The identity (certificate subject alternative name) an attestation
    /// must be signed by, e.g.
    /// `https://github.com/org/repo/.github/workflows/build.yml@refs/heads/main`.
    /// `*` matches any sequence of characters. Can be given multiple times;
    /// any match is accepted. Without this flag any valid signature is
    /// accepted.
    #[clap(
        long = "trusted-publisher",
        value_name = "IDENTITY",
        requires = "verify_attestations"
    )]
    trusted_publishers: Vec<String>,

    /// The OIDC issuer the signing identity must come from, e.g.
    /// `https://token.actions.githubusercontent.com` for GitHub Actions.
    #[clap(long, value_name = "URL", requires = "verify_attestations")]
    trusted_issuer: Option<String>,

    /// Accept attestations whose `targetChannel` differs from the channel the
    /// package was retrieved from, e.g. when installing from a mirror.
    #[clap(long, requires = "verify_attestations")]
    allow_channel_mismatch: bool,
}

impl AttestationArgs {
    /// Builds the verification policy described by the flags.
    pub fn policy(&self) -> VerificationPolicy {
        let Some(mode) = self.verify_attestations else {
            return VerificationPolicy::Disabled;
        };

        let issuer = self.trusted_issuer.as_deref().map(Issuer::new);
        let publishers: Vec<Publisher> = if self.trusted_publishers.is_empty() {
            let publisher = Publisher::new();
            vec![match &issuer {
                Some(issuer) => publisher.with_issuer(issuer.clone()),
                None => publisher,
            }]
        } else {
            self.trusted_publishers
                .iter()
                .map(|identity| {
                    let publisher = Publisher::new().with_identity(identity.as_str());
                    match &issuer {
                        Some(issuer) => publisher.with_issuer(issuer.clone()),
                        None => publisher,
                    }
                })
                .collect()
        };

        let config = VerificationConfig::new()
            .with_default_publishers(publishers)
            .with_channel_check(if self.allow_channel_mismatch {
                ChannelCheck::Warn
            } else {
                ChannelCheck::Require
            });

        match mode {
            VerifyMode::Require => VerificationPolicy::Require(config),
            VerifyMode::Warn => VerificationPolicy::Warn(config),
        }
    }
}
