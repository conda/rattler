use std::time::Duration;

use clap::Parser;

use crate::{AzureCliSasError, AzureCredentials, AzureUrlError, mint_user_delegation_sas};

/// Default lifetime, in minutes, of a SAS minted from an `az login` session.
///
/// SAS tokens are deliberately short-lived: a SAS cannot be individually revoked,
/// so a short lifetime keeps the blast radius small if one leaks. Thirty minutes
/// comfortably covers a typical index or upload run.
const DEFAULT_AZURE_CLI_SAS_TTL_MINUTES: u64 = 30;

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AzureAuthSource {
    /// Use a shared storage account key verbatim.
    AccountKey(String),

    /// Use a supplied SAS token verbatim.
    SasToken(String),

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
    /// `permissions` and `cli_context` are consulted **only** for the
    /// [`AzureAuthSource::AzureCli`] arm, which mints a SAS scoped to the
    /// container returned by `cli_context` with those permissions. The account
    /// key and SAS token arms never invoke `cli_context`, so callers pay for
    /// account/container derivation only on the minting path.
    pub fn resolve(
        self,
        permissions: &str,
        cli_context: impl FnOnce() -> Result<(String, String), AzureCredentialsError>,
    ) -> Result<AzureCredentials, AzureCredentialsError> {
        match self {
            AzureAuthSource::AccountKey(key) => Ok(AzureCredentials::AccountKey(key)),
            AzureAuthSource::SasToken(token) => Ok(AzureCredentials::SasToken(token)),
            AzureAuthSource::AzureCli { ttl } => {
                let (account, container) = cli_context()?;
                let token = mint_user_delegation_sas(&account, &container, permissions, ttl)?;
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
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct AzureCredentialsOpts {
    /// The Azure Storage account key.
    #[arg(long, env = "AZURE_STORAGE_KEY", help_heading = "Azure Credentials")]
    pub account_key: Option<String>,

    /// A shared access signature (SAS) token, with or without a leading `?`.
    #[arg(
        long,
        env = "AZURE_STORAGE_SAS_TOKEN",
        help_heading = "Azure Credentials"
    )]
    pub sas_token: Option<String>,

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
        help_heading = "Azure Credentials"
    )]
    pub azure_cli_sas_ttl_minutes: u64,
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
                ttl: Duration::from_secs(self.azure_cli_sas_ttl_minutes * 60),
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
    /// Precedence is applied by [`AzureCredentialsOpts::source`]. `permissions`
    /// and `cli_context` are consulted only when the winning source is
    /// `--azure-cli`; see [`AzureAuthSource::resolve`].
    pub fn resolve(
        self,
        permissions: &str,
        cli_context: impl FnOnce() -> Result<(String, String), AzureCredentialsError>,
    ) -> Result<AzureCredentials, AzureCredentialsError> {
        self.source()?.resolve(permissions, cli_context)
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
            account_key: account_key.map(str::to_string),
            sas_token: sas_token.map(str::to_string),
            azure_cli,
            azure_cli_sas_ttl_minutes: DEFAULT_AZURE_CLI_SAS_TTL_MINUTES,
        }
    }

    /// `cli_context` must not be invoked for the account-key/SAS-token paths.
    fn unreachable_context() -> Result<(String, String), AzureCredentialsError> {
        panic!("cli_context should not be called for non-azure-cli sources");
    }

    #[test]
    fn account_key_resolves() {
        assert!(matches!(
            opts(Some("key"), None, false).resolve("cw", unreachable_context),
            Ok(AzureCredentials::AccountKey(k)) if k == "key"
        ));
    }

    #[test]
    fn sas_token_resolves() {
        assert!(matches!(
            opts(None, Some("sv=..."), false).resolve("cw", unreachable_context),
            Ok(AzureCredentials::SasToken(t)) if t == "sv=..."
        ));
    }

    #[test]
    fn none_is_rejected() {
        assert!(matches!(
            opts(None, None, false).resolve("cw", unreachable_context),
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
            Ok(AzureAuthSource::SasToken(t)) if t == "sv=..."
        ));
        // Account key is the last resort.
        assert!(matches!(
            opts(Some("key"), None, false).source(),
            Ok(AzureAuthSource::AccountKey(k)) if k == "key"
        ));
    }

    #[test]
    fn azure_cli_ttl_is_carried_through() {
        let mut opts = opts(None, None, true);
        opts.azure_cli_sas_ttl_minutes = 45;
        assert_eq!(
            opts.source().unwrap(),
            AzureAuthSource::AzureCli {
                ttl: Duration::from_secs(45 * 60),
            }
        );
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
}
