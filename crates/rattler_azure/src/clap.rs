use std::time::Duration;

use clap::Parser;

use secrecy::{ExposeSecret, SecretString};

use crate::{
    AzureCliSasError, AzureCredentials, AzureLocation, AzureScheme, AzureUrlError,
    mint_user_delegation_sas,
};

/// A SAS cannot be individually revoked, so use a short lifetime
const DEFAULT_AZURE_CLI_SAS_TTL_MINUTES: u64 = 30;

/// Azure caps the lifetime of a user-delegation key here
const MAX_AZURE_CLI_SAS_TTL_MINUTES: u64 = 7 * 24 * 60;

#[derive(Debug, thiserror::Error)]
pub enum AzureCredentialsError {
    #[error("no Azure credentials supplied: pass --account-key, --sas-token, or --azure-cli")]
    Missing,

    #[error(transparent)]
    Url(#[from] AzureUrlError),

    #[error("failed to mint a user-delegation SAS from the Azure CLI")]
    Cli(#[from] AzureCliSasError),
}

#[derive(Clone, Debug)]
pub enum AzureAuthSource {
    AccountKey(SecretString),

    SasToken(SecretString),

    AzureCli { ttl: Duration },
}

impl AzureAuthSource {
    pub async fn resolve(
        self,
        permissions: &str,
        location: &AzureLocation,
        scheme: AzureScheme,
    ) -> Result<AzureCredentials, AzureCredentialsError> {
        match self {
            AzureAuthSource::AccountKey(account_key) => {
                Ok(AzureCredentials::AccountKey(account_key))
            }
            AzureAuthSource::SasToken(token) => Ok(AzureCredentials::SasToken(token)),
            AzureAuthSource::AzureCli { ttl } => {
                let (key, container) = location.addressed()?;
                let token =
                    mint_user_delegation_sas(key.account(), container, permissions, ttl, scheme)
                        .await?;
                Ok(AzureCredentials::SasToken(token))
            }
        }
    }
}

#[derive(Clone, Debug, Parser)]
pub struct AzureCredentialsOpts {
    /// The Azure Storage account key.
    ///
    /// Mutually exclusive with `--sas-token` and `--azure-cli`.
    #[arg(
        long,
        env = "AZURE_STORAGE_KEY",
        help_heading = "Azure Credentials",
        value_parser = secret,
        group = "credential"
    )]
    pub account_key: Option<SecretString>,

    /// A shared access signature (SAS) token, with or without a leading `?`.
    ///
    /// Mutually exclusive with `--account-key` and `--azure-cli`.
    #[arg(
        long,
        env = "AZURE_STORAGE_SAS_TOKEN",
        help_heading = "Azure Credentials",
        value_parser = secret,
        group = "credential"
    )]
    pub sas_token: Option<SecretString>,

    /// Mint a short-lived user-delegation SAS from the current `az login`
    /// session (requires the Azure CLI).
    ///
    /// Mutually exclusive with `--account-key` and `--sas-token`.
    #[allow(clippy::doc_markdown)]
    #[arg(long, help_heading = "Azure Credentials", group = "credential")]
    pub azure_cli: bool,

    /// Lifetime, in minutes, of the SAS minted for `--azure-cli`.
    ///
    /// Raise it for very large index or upload runs. If the SAS expires mid-run,
    /// later requests fail with a 403 and the run aborts, possibly leaving the
    /// channel in a broken state.
    #[arg(
        long,
        default_value_t = DEFAULT_AZURE_CLI_SAS_TTL_MINUTES,
        value_parser = clap::value_parser!(u64).range(1..=MAX_AZURE_CLI_SAS_TTL_MINUTES),
        help_heading = "Azure Credentials"
    )]
    pub azure_cli_sas_ttl_minutes: u64,
}

fn secret(value: &str) -> Result<SecretString, std::convert::Infallible> {
    Ok(value.into())
}

impl PartialEq for AzureCredentialsOpts {
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

    pub async fn resolve(
        self,
        permissions: &str,
        location: &AzureLocation,
        scheme: AzureScheme,
    ) -> Result<AzureCredentials, AzureCredentialsError> {
        self.source()?.resolve(permissions, location, scheme).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        creds: AzureCredentialsOpts,
    }

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

    fn unaddressable() -> AzureLocation {
        let channel = crate::AzureChannelUrl::parse("az://acct.blob.core.windows.net").unwrap();
        let location = crate::locate_as(&channel, crate::AzureAddressing::HostStyle).unwrap();
        assert!(location.addressed().is_err());
        location
    }

    #[rstest::rstest]
    #[case::account_key(Some("key"), None)]
    #[case::sas_token(None, Some("sv=..."))]
    #[case::missing(None, None)]
    #[tokio::test]
    async fn given_credentials_resolve(
        #[case] account_key: Option<&str>,
        #[case] sas_token: Option<&str>,
    ) {
        let resolved = opts(account_key, sas_token, false)
            .resolve("cw", &unaddressable(), AzureScheme::Https)
            .await;

        match (account_key, sas_token) {
            (Some(k), None) => assert!(matches!(
                resolved, Ok(AzureCredentials::AccountKey(s)) if s.expose_secret() == k
            )),
            (None, Some(t)) => assert!(matches!(
                resolved, Ok(AzureCredentials::SasToken(s)) if s.expose_secret() == t
            )),
            (None, None) => assert!(matches!(resolved, Err(AzureCredentialsError::Missing))),
            (Some(_), Some(_)) => unreachable!("no such case"),
        }
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

    // cannot mint anything here as there is no `az` cli
    #[test]
    fn azure_cli_flag_and_ttl_parse() {
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

        let out = format!("{:?}", opts(Some("supersecretkey"), None, false));
        assert!(out.contains("REDACTED"), "not redacted: {out}");
        assert!(!out.contains("supersecret"), "leaked key: {out}");

        let out = format!("{:?}", opts(None, Some("sig=deadbeef"), false));
        assert!(out.contains("REDACTED"), "not redacted: {out}");
        assert!(!out.contains("deadbeef"), "leaked token: {out}");
    }

    #[test]
    fn ttl_range_is_enforced() {
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
