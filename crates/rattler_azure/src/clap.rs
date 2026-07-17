use clap::Parser;

use crate::{AzureCredentials, MissingAzureCredentials};

/// Manually specified Azure Blob credentials.
///
/// See [`super::AzureCredentials`] for details on how these credentials are
/// used. `--account-key` and `--sas-token` are mutually exclusive; exactly one
/// must be supplied.
#[derive(Clone, Debug, PartialEq, Parser)]
pub struct AzureCredentialsOpts {
    /// The Azure Storage account key.
    #[arg(
        long,
        env = "AZURE_STORAGE_KEY",
        conflicts_with = "sas_token",
        help_heading = "Azure Credentials"
    )]
    pub account_key: Option<String>,

    /// A shared access signature (SAS) token, with or without a leading `?`.
    #[arg(
        long,
        env = "AZURE_STORAGE_SAS_TOKEN",
        conflicts_with = "account_key",
        help_heading = "Azure Credentials"
    )]
    pub sas_token: Option<String>,
}

impl TryFrom<AzureCredentialsOpts> for AzureCredentials {
    type Error = MissingAzureCredentials;

    fn try_from(value: AzureCredentialsOpts) -> Result<Self, Self::Error> {
        // `conflicts_with` guarantees at most one of the two is set, so the
        // order of these checks doesn't matter.
        if let Some(account_key) = value.account_key {
            Ok(AzureCredentials::AccountKey(account_key))
        } else if let Some(sas_token) = value.sas_token {
            Ok(AzureCredentials::SasToken(sas_token))
        } else {
            Err(MissingAzureCredentials)
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
        };
        assert!(matches!(
            AzureCredentials::try_from(opts),
            Ok(AzureCredentials::AccountKey(k)) if k == "key"
        ));
    }

    #[test]
    fn sas_token_resolves() {
        let opts = AzureCredentialsOpts {
            account_key: None,
            sas_token: Some("sv=...".into()),
        };
        assert!(matches!(
            AzureCredentials::try_from(opts),
            Ok(AzureCredentials::SasToken(t)) if t == "sv=..."
        ));
    }

    #[test]
    fn neither_is_rejected() {
        let opts = AzureCredentialsOpts {
            account_key: None,
            sas_token: None,
        };
        assert!(AzureCredentials::try_from(opts).is_err());
    }
}
