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
    /// `*` matches any sequence of characters. This constraint applies to all
    /// packages, regardless of channel. Without this flag any identity is accepted,
    /// subject to `--trusted-issuer` when supplied.
    #[clap(
        long = "trusted-publisher",
        value_name = "IDENTITY",
        requires = "verify_attestations"
    )]
    trusted_publisher: Option<String>,

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

        let mut publisher = Publisher::new();
        if let Some(identity) = &self.trusted_publisher {
            publisher = publisher.with_identity(identity.as_str());
        }
        if let Some(issuer) = &self.trusted_issuer {
            publisher = publisher.with_issuer(Issuer::new(issuer));
        }

        let config =
            VerificationConfig::new(publisher).with_channel_check(if self.allow_channel_mismatch {
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[clap(flatten)]
        attestations: AttestationArgs,
    }

    #[test]
    fn one_publisher_requires_both_identity_and_issuer() {
        let cli = Cli::try_parse_from([
            "test",
            "--verify-attestations",
            "require",
            "--trusted-publisher",
            "https://github.com/org/*",
            "--trusted-issuer",
            "https://token.actions.githubusercontent.com",
        ])
        .unwrap();
        let policy = cli.attestations.policy();
        assert!(policy.is_required());
        let publisher = policy.config().unwrap().publisher();
        let identity = Some("https://github.com/org/repo/workflow");
        let issuer = Some("https://token.actions.githubusercontent.com");
        assert!(publisher.matches(identity, issuer));
        assert!(!publisher.matches(Some("https://github.com/other/repo/workflow"), issuer));
        assert!(!publisher.matches(identity, Some("https://gitlab.com")));
    }

    #[test]
    fn publisher_flag_is_not_repeatable() {
        assert!(
            Cli::try_parse_from([
                "test",
                "--verify-attestations",
                "require",
                "--trusted-publisher",
                "alice",
                "--trusted-publisher",
                "bob",
            ])
            .is_err()
        );
    }

    #[test]
    fn optional_constraints_and_verification_mode() {
        let cli = Cli::try_parse_from(["test"]).unwrap();
        assert!(!cli.attestations.policy().is_enabled());

        let cli = Cli::try_parse_from([
            "test",
            "--verify-attestations",
            "warn",
            "--allow-channel-mismatch",
            "--trusted-issuer",
            "https://gitlab.com",
        ])
        .unwrap();
        let policy = cli.attestations.policy();
        assert!(!policy.is_required());
        let config = policy.config().unwrap();
        assert_eq!(config.channel_check(), ChannelCheck::Warn);
        assert!(
            config
                .publisher()
                .matches(Some("any identity"), Some("https://gitlab.com"))
        );
        assert!(
            !config
                .publisher()
                .matches(Some("any identity"), Some("other issuer"))
        );

        let cli = Cli::try_parse_from(["test", "--verify-attestations", "require"]).unwrap();
        assert!(
            cli.attestations
                .policy()
                .config()
                .unwrap()
                .publisher()
                .matches(None, None)
        );
    }
}
