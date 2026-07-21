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

/// Errors that can occur while minting a user-delegation SAS via the Azure CLI.
#[cfg(feature = "azure-cli-sas")]
#[derive(Debug, thiserror::Error)]
pub enum AzureCliSasError {
    /// The SAS expiry timestamp could not be computed.
    #[error("failed to compute the SAS expiry timestamp: {0}")]
    Expiry(String),

    /// The `az` executable could not be found on `PATH`.
    #[error("could not find the Azure CLI (`az`) on PATH; install it and run `az login`")]
    AzNotFound(#[source] std::io::Error),

    /// The `az` process could not be spawned.
    #[error("failed to run the Azure CLI (`az`)")]
    Spawn(#[source] std::io::Error),

    /// `az` exited with a non-zero status.
    #[error("the Azure CLI failed to generate a user-delegation SAS (is `az login` current?): {0}")]
    CommandFailed(String),

    /// `az` succeeded but produced no SAS token.
    #[error("the Azure CLI returned an empty SAS token")]
    EmptyOutput,
}

/// Mint a short-lived user-delegation SAS for a container by shelling out to the
/// Azure CLI.
///
/// opendal's azblob backend (used by the index and upload write paths) only
/// accepts a shared account key or a SAS token, not an AAD bearer token. To let
/// users authenticate writes with their `az login` session, this converts that
/// session into a SAS via:
///
/// ```text
/// az storage container generate-sas --account-name <account> --name <container>
///     --permissions <permissions> --expiry <expiry> --auth-mode login --as-user
///     --https-only -o tsv
/// ```
///
/// `permissions` is the Azure SAS permission string (e.g. `"cw"`). The returned
/// token has no leading `?`. Requires `az` on `PATH` and a prior `az login`.
///
/// This blocks the calling thread while the `az` process runs; it is meant to be
/// called once at setup time.
#[cfg(feature = "azure-cli-sas")]
pub fn mint_user_delegation_sas(
    account: &str,
    container: &str,
    permissions: &str,
    valid_for: std::time::Duration,
) -> Result<String, AzureCliSasError> {
    let signed = jiff::SignedDuration::try_from(valid_for)
        .map_err(|err| AzureCliSasError::Expiry(err.to_string()))?;
    let expiry = jiff::Timestamp::now()
        .checked_add(signed)
        .map_err(|err| AzureCliSasError::Expiry(err.to_string()))?;
    // `az` expects an ISO-8601 UTC timestamp; minute precision is sufficient.
    let expiry = expiry.strftime("%Y-%m-%dT%H:%MZ").to_string();

    let output = std::process::Command::new("az")
        .args([
            "storage",
            "container",
            "generate-sas",
            "--account-name",
            account,
            "--name",
            container,
            "--permissions",
            permissions,
            "--expiry",
            &expiry,
            "--auth-mode",
            "login",
            "--as-user",
            "--https-only",
            "-o",
            "tsv",
        ])
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                AzureCliSasError::AzNotFound(err)
            } else {
                AzureCliSasError::Spawn(err)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AzureCliSasError::CommandFailed(stderr));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(AzureCliSasError::EmptyOutput);
    }
    Ok(token)
}
