use std::time::Duration;

use clap::Parser;

use crate::{AzureCliSasError, AzureCredentials, mint_user_delegation_sas};

/// How long a SAS minted from an `az login` session stays valid.
///
/// SAS tokens are deliberately short-lived: a SAS cannot be individually
/// revoked, so a short lifetime keeps the blast radius small if one leaks. Thirty
/// minutes comfortably covers an index or upload run.
const AZURE_CLI_SAS_TTL: Duration = Duration::from_secs(30 * 60);

/// Errors that can occur while resolving [`AzureCredentialsOpts`] into
/// [`AzureCredentials`].
#[derive(Debug, thiserror::Error)]
pub enum AzureCredentialsError {
    /// No credential source was supplied.
    #[error("no Azure credentials supplied: pass --account-key, --sas-token, or --azure-cli")]
    Missing,

    /// Minting a SAS via the Azure CLI failed.
    #[error("failed to mint a user-delegation SAS from the Azure CLI")]
    Cli(#[from] AzureCliSasError),
}

/// Manually specified Azure Blob credentials.
///
/// See [`super::AzureCredentials`] for details on how these credentials are
/// used. `--account-key`, `--sas-token`, and `--azure-cli` are mutually
/// exclusive; exactly one must be supplied.
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct AzureCredentialsOpts {
    /// The Azure Storage account key.
    #[arg(
        long,
        env = "AZURE_STORAGE_KEY",
        conflicts_with_all = ["sas_token", "azure_cli"],
        help_heading = "Azure Credentials"
    )]
    pub account_key: Option<String>,

    /// A shared access signature (SAS) token, with or without a leading `?`.
    #[arg(
        long,
        env = "AZURE_STORAGE_SAS_TOKEN",
        conflicts_with_all = ["account_key", "azure_cli"],
        help_heading = "Azure Credentials"
    )]
    pub sas_token: Option<String>,

    /// Mint a short-lived user-delegation SAS from the current `az login`
    /// session (requires the Azure CLI).
    #[arg(
        long,
        conflicts_with_all = ["account_key", "sas_token"],
        help_heading = "Azure Credentials"
    )]
    pub azure_cli: bool,
}

impl AzureCredentialsOpts {
    /// Resolve the supplied options into a concrete [`AzureCredentials`].
    ///
    /// `account`, `container`, and `permissions` are only used by the
    /// `--azure-cli` path, which mints a short-lived user-delegation SAS scoped
    /// to that container with those permissions. The account and container are
    /// derived by the caller from the channel URL.
    pub fn resolve(
        self,
        account: &str,
        container: &str,
        permissions: &str,
    ) -> Result<AzureCredentials, AzureCredentialsError> {
        // `conflicts_with_all` guarantees at most one source is set, so the
        // order of these checks doesn't matter.
        if let Some(account_key) = self.account_key {
            Ok(AzureCredentials::AccountKey(account_key))
        } else if let Some(sas_token) = self.sas_token {
            Ok(AzureCredentials::SasToken(sas_token))
        } else if self.azure_cli {
            let token =
                mint_user_delegation_sas(account, container, permissions, AZURE_CLI_SAS_TTL)?;
            Ok(AzureCredentials::SasToken(token))
        } else {
            Err(AzureCredentialsError::Missing)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_key_resolves() {
        let opts = AzureCredentialsOpts {
            account_key: Some("key".into()),
            sas_token: None,
            azure_cli: false,
        };
        assert!(matches!(
            opts.resolve("acct", "container", "cw"),
            Ok(AzureCredentials::AccountKey(k)) if k == "key"
        ));
    }

    #[test]
    fn sas_token_resolves() {
        let opts = AzureCredentialsOpts {
            account_key: None,
            sas_token: Some("sv=...".into()),
            azure_cli: false,
        };
        assert!(matches!(
            opts.resolve("acct", "container", "cw"),
            Ok(AzureCredentials::SasToken(t)) if t == "sv=..."
        ));
    }

    #[test]
    fn none_is_rejected() {
        let opts = AzureCredentialsOpts {
            account_key: None,
            sas_token: None,
            azure_cli: false,
        };
        assert!(matches!(
            opts.resolve("acct", "container", "cw"),
            Err(AzureCredentialsError::Missing)
        ));
    }

    // The `--azure-cli` variant shells out to `az`, which isn't available in the
    // test environment, so we only assert the flag is wired through the parser
    // (its conflicts are enforced by clap, exercised below) rather than invoking
    // the mint path.
    #[test]
    fn azure_cli_flag_parses() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            creds: AzureCredentialsOpts,
        }

        let cli = Cli::try_parse_from(["test", "--azure-cli"]).expect("should parse");
        assert!(cli.creds.azure_cli);
        assert!(cli.creds.account_key.is_none());
        assert!(cli.creds.sas_token.is_none());
    }

    #[test]
    fn azure_cli_conflicts_with_other_sources() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            creds: AzureCredentialsOpts,
        }

        assert!(Cli::try_parse_from(["test", "--azure-cli", "--account-key", "k"]).is_err());
        assert!(Cli::try_parse_from(["test", "--azure-cli", "--sas-token", "s"]).is_err());
    }
}
