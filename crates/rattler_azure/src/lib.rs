#[cfg(feature = "clap")]
pub mod clap;

/// Credentials for authenticating to Azure Blob storage.
///
/// Exactly one authentication method is carried, so the ambiguous "both a key
/// and a SAS token" and "neither" states are unrepresentable. The storage
/// account name, endpoint, and container are not stored here: they are fully
/// determined by the channel URL (`https://<account>.blob.core.windows.net/<container>/...`)
/// and derived by the consumer.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AzureCredentials {
    /// A shared storage account key.
    AccountKey(String),

    /// A shared access signature (SAS) token.
    SasToken(String),
}

/// Error returned when no Azure credentials were supplied.
#[derive(Debug, thiserror::Error)]
#[error("no Azure credentials supplied: pass either an account key or a SAS token")]
pub struct MissingAzureCredentials;
