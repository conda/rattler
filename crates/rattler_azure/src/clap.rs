use std::time::Duration;

use clap::Parser;

use secrecy::{ExposeSecret, SecretString};

use crate::{
    AzureChannelUrl, AzureCliSasError, AzureCoordinates, AzureCredentials, AzureEndpoint,
    AzureUrlError, account_and_container, mint_user_delegation_sas,
};

/// Default lifetime, in minutes, of a SAS minted from an `az login` session.
///
/// SAS tokens are deliberately short-lived: a SAS cannot be individually revoked,
/// so a short lifetime keeps the blast radius small if one leaks. Thirty minutes
/// comfortably covers a typical index or upload run.
const DEFAULT_AZURE_CLI_SAS_TTL_MINUTES: u64 = 30;

/// Upper bound, in minutes, accepted for `--azure-cli-sas-ttl-minutes`.
///
/// A SAS is meant to be short-lived; one week is already generous. Capping the
/// value at the clap layer also keeps `minutes * 60` well clear of overflowing
/// the [`Duration`] arithmetic in [`AzureCredentialsOpts::source`].
const MAX_AZURE_CLI_SAS_TTL_MINUTES: u64 = 7 * 24 * 60;

/// Errors that can occur while resolving [`AzureCredentialsOpts`] into
/// [`AzureCredentials`].
#[derive(Debug, thiserror::Error)]
pub enum AzureCredentialsError {
    /// No credential source was supplied.
    #[error("no Azure credentials supplied: pass --account-key, --sas-token, or --azure-cli")]
    Missing,

    /// The channel URL required to mint a SAS could not be parsed.
    #[error(transparent)]
    Url(#[from] AzureUrlError),

    /// Minting a SAS via the Azure CLI failed.
    #[error("failed to mint a user-delegation SAS from the Azure CLI")]
    Cli(#[from] AzureCliSasError),
}

/// A resolved, unambiguous choice of authentication source.
///
/// [`AzureCredentialsOpts`] can express several inputs at once (an exported
/// `AZURE_STORAGE_KEY` and an explicit `--azure-cli`, say); this enum is the
/// single winner after precedence is applied, so downstream code never has to
/// reason about combinations. Only [`AzureAuthSource::AzureCli`] carries state
/// (the minting TTL), which is why account/container derivation is needed for
/// that arm alone.
#[derive(Clone, Debug)]
pub enum AzureAuthSource {
    /// Use a shared storage account key verbatim.
    AccountKey(SecretString),

    /// Use a supplied SAS token verbatim.
    SasToken(SecretString),

    /// Mint a short-lived user-delegation SAS from the current `az login`
    /// session, valid for `ttl`.
    AzureCli {
        /// How long the minted SAS should remain valid.
        ttl: Duration,
    },
}

impl AzureAuthSource {
    /// Resolve this source into concrete [`AzureCredentials`].
    ///
    /// `permissions`, `channel` and `endpoint` are consulted **only** for the
    /// [`AzureAuthSource::AzureCli`] arm, which mints a SAS scoped to the channel's
    /// container with those permissions. Taking the channel and its endpoint rather
    /// than pre-derived coordinates is what keeps the account the SAS is minted for
    /// and the scheme it is restricted to from coming from two different places.
    pub async fn resolve(
        self,
        permissions: &str,
        channel: &AzureChannelUrl,
        endpoint: AzureEndpoint,
    ) -> Result<AzureCredentials, AzureCredentialsError> {
        match self {
            AzureAuthSource::AccountKey(key) => Ok(AzureCredentials::AccountKey(key)),
            AzureAuthSource::SasToken(token) => Ok(AzureCredentials::SasToken(token)),
            AzureAuthSource::AzureCli { ttl } => {
                let AzureCoordinates { account, container } =
                    account_and_container(channel, endpoint.addressing)?;
                let token = mint_user_delegation_sas(
                    &account,
                    &container,
                    permissions,
                    ttl,
                    endpoint.scheme,
                )
                .await?;
                Ok(AzureCredentials::SasToken(token))
            }
        }
    }
}

/// Manually specified Azure Blob credentials.
///
/// See [`super::AzureCredentials`] for details on how these credentials are used.
/// Several inputs may be present at once (for example when `AZURE_STORAGE_KEY` is
/// exported *and* `--azure-cli` is passed), so [`AzureCredentialsOpts::source`]
/// applies an explicit precedence rather than treating the combination as an
/// error — see that method for the exact ordering.
#[derive(Clone, Debug, Parser)]
pub struct AzureCredentialsOpts {
    /// The Azure Storage account key.
    ///
    /// Mutually exclusive with `--sas-token`: supplying both is a usage error
    /// rather than silently discarding one. `--azure-cli` layers on top of both
    /// (see [`AzureCredentialsOpts::source`]).
    #[arg(
        long,
        env = "AZURE_STORAGE_KEY",
        conflicts_with = "sas_token",
        help_heading = "Azure Credentials",
        value_parser = secret
    )]
    pub account_key: Option<SecretString>,

    /// A shared access signature (SAS) token, with or without a leading `?`.
    #[arg(
        long,
        env = "AZURE_STORAGE_SAS_TOKEN",
        help_heading = "Azure Credentials",
        value_parser = secret
    )]
    pub sas_token: Option<SecretString>,

    /// Mint a short-lived user-delegation SAS from the current `az login`
    /// session (requires the Azure CLI).
    ///
    /// Takes precedence over AZURE_STORAGE_KEY / AZURE_STORAGE_SAS_TOKEN, so it
    /// can be used to override ambient credentials picked up from the
    /// environment.
    #[allow(clippy::doc_markdown)]
    #[arg(long, help_heading = "Azure Credentials")]
    pub azure_cli: bool,

    /// Lifetime, in minutes, of the SAS minted for `--azure-cli`.
    ///
    /// The default keeps the token short-lived. Raise it for very large index or
    /// upload runs: if the SAS expires mid-run, subsequent requests fail with a
    /// 403 and the run aborts, potentially leaving a partial index behind.
    #[arg(
        long,
        default_value_t = DEFAULT_AZURE_CLI_SAS_TTL_MINUTES,
        value_parser = clap::value_parser!(u64).range(1..=MAX_AZURE_CLI_SAS_TTL_MINUTES),
        help_heading = "Azure Credentials"
    )]
    pub azure_cli_sas_ttl_minutes: u64,
}

/// Take a command-line or environment value straight into a [`SecretString`], so
/// it is never held as a plain `String` that a `{:?}` could reach.
fn secret(value: &str) -> Result<SecretString, std::convert::Infallible> {
    Ok(value.into())
}

impl PartialEq for AzureCredentialsOpts {
    /// Hand-written because [`SecretString`] withholds `PartialEq` — comparing
    /// secrets is not constant-time. The containing `UploadOpts` tree derives
    /// `PartialEq`, and comparing parsed command lines is not a secrets check.
    fn eq(&self, other: &Self) -> bool {
        fn same(left: Option<&SecretString>, right: Option<&SecretString>) -> bool {
            match (left, right) {
                (Some(left), Some(right)) => left.expose_secret() == right.expose_secret(),
                (None, None) => true,
                _ => false,
            }
        }

        same(self.account_key.as_ref(), other.account_key.as_ref())
            && same(self.sas_token.as_ref(), other.sas_token.as_ref())
            && self.azure_cli == other.azure_cli
            && self.azure_cli_sas_ttl_minutes == other.azure_cli_sas_ttl_minutes
    }
}

impl AzureCredentialsOpts {
    /// Collapse the supplied options into a single, unambiguous
    /// [`AzureAuthSource`].
    ///
    /// When more than one input is present the following precedence applies,
    /// highest first:
    ///
    /// 1. `--azure-cli` — an explicit opt-in, so it wins over anything picked up
    ///    from the environment.
    /// 2. `--sas-token` / `AZURE_STORAGE_SAS_TOKEN`.
    /// 3. `--account-key` / `AZURE_STORAGE_KEY`.
    ///
    /// If none are set, returns [`AzureCredentialsError::Missing`].
    pub fn source(&self) -> Result<AzureAuthSource, AzureCredentialsError> {
        if self.azure_cli {
            Ok(AzureAuthSource::AzureCli {
                ttl: Duration::from_secs(self.azure_cli_sas_ttl_minutes.saturating_mul(60)),
            })
        } else if let Some(sas_token) = &self.sas_token {
            Ok(AzureAuthSource::SasToken(sas_token.clone()))
        } else if let Some(account_key) = &self.account_key {
            Ok(AzureAuthSource::AccountKey(account_key.clone()))
        } else {
            Err(AzureCredentialsError::Missing)
        }
    }

    /// Resolve the supplied options into concrete [`AzureCredentials`].
    ///
    /// Precedence is applied by [`AzureCredentialsOpts::source`]. `permissions`,
    /// `channel` and `endpoint` are consulted only when the winning source is
    /// `--azure-cli`; see [`AzureAuthSource::resolve`].
    pub async fn resolve(
        self,
        permissions: &str,
        channel: &AzureChannelUrl,
        endpoint: AzureEndpoint,
    ) -> Result<AzureCredentials, AzureCredentialsError> {
        self.source()?.resolve(permissions, channel, endpoint).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(
        account_key: Option<&str>,
        sas_token: Option<&str>,
        azure_cli: bool,
    ) -> AzureCredentialsOpts {
        AzureCredentialsOpts {
            account_key: account_key.map(Into::into),
            sas_token: sas_token.map(Into::into),
            azure_cli,
            azure_cli_sas_ttl_minutes: DEFAULT_AZURE_CLI_SAS_TTL_MINUTES,
        }
    }

    /// A channel whose coordinates cannot be derived under `endpoint`: resolving a
    /// verbatim credential must not need them, and this is what proves it.
    fn underivable() -> (AzureChannelUrl, AzureEndpoint) {
        let channel =
            AzureChannelUrl::parse("az://127.0.0.1:10000/devstoreaccount1/general").unwrap();
        let endpoint = AzureEndpoint::default();
        assert!(account_and_container(&channel, endpoint.addressing).is_err());
        (channel, endpoint)
    }

    #[tokio::test]
    async fn account_key_resolves() {
        let (channel, endpoint) = underivable();
        assert!(matches!(
            opts(Some("key"), None, false).resolve("cw", &channel, endpoint).await,
            Ok(AzureCredentials::AccountKey(k)) if k.expose_secret() == "key"
        ));
    }

    #[tokio::test]
    async fn sas_token_resolves() {
        let (channel, endpoint) = underivable();
        assert!(matches!(
            opts(None, Some("sv=..."), false).resolve("cw", &channel, endpoint).await,
            Ok(AzureCredentials::SasToken(t)) if t.expose_secret() == "sv=..."
        ));
    }

    #[tokio::test]
    async fn none_is_rejected() {
        let (channel, endpoint) = underivable();
        assert!(matches!(
            opts(None, None, false)
                .resolve("cw", &channel, endpoint)
                .await,
            Err(AzureCredentialsError::Missing)
        ));
    }

    #[test]
    fn azure_cli_beats_sas_beats_account_key() {
        // All three present: `--azure-cli` wins.
        assert!(matches!(
            opts(Some("key"), Some("sv=..."), true).source(),
            Ok(AzureAuthSource::AzureCli { .. })
        ));
        // SAS token beats an account key when `--azure-cli` is absent.
        assert!(matches!(
            opts(Some("key"), Some("sv=..."), false).source(),
            Ok(AzureAuthSource::SasToken(t)) if t.expose_secret() == "sv=..."
        ));
        // Account key is the last resort.
        assert!(matches!(
            opts(Some("key"), None, false).source(),
            Ok(AzureAuthSource::AccountKey(k)) if k.expose_secret() == "key"
        ));
    }

    #[test]
    fn azure_cli_ttl_is_carried_through() {
        let mut opts = opts(None, None, true);
        opts.azure_cli_sas_ttl_minutes = 45;
        assert!(matches!(
            opts.source().unwrap(),
            AzureAuthSource::AzureCli { ttl } if ttl == Duration::from_secs(45 * 60)
        ));
    }

    // The `--azure-cli` resolve path shells out to `az`, which isn't available in
    // the test environment, so we only assert the flag and its TTL parse through
    // clap and that precedence selects the CLI source; the mint itself is not
    // exercised here.
    #[test]
    fn azure_cli_flag_and_ttl_parse() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            creds: AzureCredentialsOpts,
        }

        let cli = Cli::try_parse_from(["test", "--azure-cli", "--azure-cli-sas-ttl-minutes", "90"])
            .expect("should parse");
        assert!(cli.creds.azure_cli);
        assert_eq!(cli.creds.azure_cli_sas_ttl_minutes, 90);

        let default = Cli::try_parse_from(["test", "--azure-cli"]).expect("should parse");
        assert_eq!(
            default.creds.azure_cli_sas_ttl_minutes,
            DEFAULT_AZURE_CLI_SAS_TTL_MINUTES
        );
    }

    /// `--account-key` and `--sas-token` are mutually exclusive: passing both is
    /// a clap error rather than silently discarding one.
    #[test]
    fn account_key_and_sas_token_conflict() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            creds: AzureCredentialsOpts,
        }

        let err = Cli::try_parse_from(["test", "--account-key", "k", "--sas-token", "sv=..."])
            .map(|_| ())
            .expect_err("passing both --account-key and --sas-token must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    /// Neither the resolved source nor the raw options may print a secret.
    #[test]
    fn debug_never_prints_secrets() {
        let sources = [
            AzureAuthSource::AccountKey("supersecretkey".into()),
            AzureAuthSource::SasToken("sig=deadbeef".into()),
        ];
        for source in &sources {
            let out = format!("{source:?}");
            assert!(out.contains("REDACTED"), "not redacted: {out}");
            assert!(!out.contains("supersecret"), "leaked key: {out}");
            assert!(!out.contains("deadbeef"), "leaked token: {out}");
        }

        // The TTL arm carries no secret, so it stays fully printable.
        let cli = AzureAuthSource::AzureCli {
            ttl: Duration::from_secs(60),
        };
        assert!(format!("{cli:?}").contains("60"));

        let out = format!("{:?}", opts(Some("supersecretkey"), None, false));
        assert!(out.contains("REDACTED"), "not redacted: {out}");
        assert!(!out.contains("supersecret"), "leaked key: {out}");

        let out = format!("{:?}", opts(None, Some("sig=deadbeef"), false));
        assert!(out.contains("REDACTED"), "not redacted: {out}");
        assert!(!out.contains("deadbeef"), "leaked token: {out}");

        // Absent secrets print as `None`, so the redaction cannot be mistaken for
        // a supplied-but-hidden value.
        let out = format!("{:?}", opts(None, None, true));
        assert!(out.contains("account_key: None"), "unexpected: {out}");
        assert!(out.contains("sas_token: None"), "unexpected: {out}");
    }

    /// A zero TTL is rejected, and the maximum is capped so `minutes * 60`
    /// cannot overflow.
    #[test]
    fn ttl_range_is_enforced() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            creds: AzureCredentialsOpts,
        }

        assert!(
            Cli::try_parse_from(["test", "--azure-cli", "--azure-cli-sas-ttl-minutes", "0"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "test",
                "--azure-cli",
                "--azure-cli-sas-ttl-minutes",
                &(MAX_AZURE_CLI_SAS_TTL_MINUTES + 1).to_string(),
            ])
            .is_err()
        );
    }
}
