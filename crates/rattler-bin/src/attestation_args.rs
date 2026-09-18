//! Command-line flags that configure Sigstore attestation verification for
//! commands that install packages.

use clap::ArgGroup;
use rattler_sigstore::{ChannelCheck, VerificationConfig, VerificationPolicy};

use crate::publisher_args::PublisherArgs;

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
#[clap(group(
    ArgGroup::new("publisher_constraints")
        .args(["identity", "issuer"])
        .requires("verify_attestations")
        .multiple(true)
))]
pub struct AttestationArgs {
    /// Verify the Sigstore attestations of the packages being installed.
    /// Attestations are discovered through the `attestations_sha256` field of
    /// the repodata. `require` fails the installation when a package has no
    /// valid attestation, `warn` only reports problems.
    #[clap(long, value_name = "MODE", value_enum)]
    verify_attestations: Option<VerifyMode>,

    /// Signing certificate publisher constraints.
    #[clap(flatten)]
    publisher: PublisherArgs,

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
    fn publisher_constraints_are_applied() {
        let cli = Cli::try_parse_from([
            "test",
            "--verify-attestations",
            "require",
            "--identity",
            "https://github.com/org/*",
            "--issuer",
            "github",
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
                "--identity",
                "alice",
                "--identity",
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
            "--issuer",
            "gitlab",
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

    #[test]
    fn publisher_constraints_require_verification_mode() {
        for flag in ["--identity", "--issuer"] {
            assert!(Cli::try_parse_from(["test", flag, "github"]).is_err());
        }
    }
}
