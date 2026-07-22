#[cfg(feature = "clap")]
pub mod clap;

use url::Url;

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

/// Strip a single leading `?` from a SAS token.
///
/// `--sas-token` may be supplied with or without a leading `?`, but a SAS minted
/// by [`mint_user_delegation_sas`] never has one. Normalizing at the single point
/// where a token is handed to opendal means both sources behave identically.
pub fn normalize_sas_token(token: &str) -> &str {
    token.strip_prefix('?').unwrap_or(token)
}

/// Errors that can occur while deriving Azure Blob coordinates from a channel
/// URL.
#[derive(Debug, thiserror::Error)]
pub enum AzureUrlError {
    /// The URL has no host component.
    #[error("no host in Azure blob URL")]
    NoHost,

    /// The host is not a dotted domain, so no storage account can be derived.
    #[error(
        "Azure blob URL host `{0}` is not a dotted domain of the form `<account>.blob.<suffix>`; \
         IP literals and single-label hosts have no derivable storage account"
    )]
    InvalidHost(String),

    /// The account name (first host label) is empty.
    #[error("could not derive account name from Azure blob URL")]
    NoAccount,

    /// The URL has no container path segment.
    #[error("no container in Azure blob URL")]
    NoContainer,
}

/// Derive the storage account name and container from an Azure Blob channel URL
/// of the form `https://<account>.blob.core.windows.net/<container>/<prefix>`.
///
/// The account name is the first label of the host, so the host must be a dotted
/// domain; IP-literal and single-label hosts (e.g. `localhost` or the Azurite
/// emulator) are rejected because no account can be derived from them.
pub fn account_and_container(url: &Url) -> Result<(String, String), AzureUrlError> {
    let host = url.host_str().ok_or(AzureUrlError::NoHost)?;
    if !matches!(url.host(), Some(url::Host::Domain(domain)) if domain.contains('.')) {
        return Err(AzureUrlError::InvalidHost(host.to_string()));
    }
    let account = host
        .split('.')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or(AzureUrlError::NoAccount)?;
    let container = url
        .path_segments()
        .and_then(|mut segments| segments.next())
        .filter(|segment| !segment.is_empty())
        .ok_or(AzureUrlError::NoContainer)?;
    Ok((account.to_string(), container.to_string()))
}

/// Errors that can occur while minting a user-delegation SAS via the Azure CLI.
#[cfg(feature = "clap")]
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
#[cfg(feature = "clap")]
pub fn mint_user_delegation_sas(
    account: &str,
    container: &str,
    permissions: &str,
    valid_for: std::time::Duration,
) -> Result<String, AzureCliSasError> {
    /// Extra slack added to the requested lifetime so a slightly fast client
    /// clock (the expiry is computed from *this* machine's time) does not shrink
    /// the usable window toward zero at the Azure end.
    const CLOCK_SKEW_HEADROOM: std::time::Duration = std::time::Duration::from_secs(120);

    let signed = jiff::SignedDuration::try_from(valid_for.saturating_add(CLOCK_SKEW_HEADROOM))
        .map_err(|err| AzureCliSasError::Expiry(err.to_string()))?;
    let expiry = jiff::Timestamp::now()
        .checked_add(signed)
        .map_err(|err| AzureCliSasError::Expiry(err.to_string()))?;
    // `az` expects an ISO-8601 UTC timestamp; keep second precision so the window
    // is not floored down to the enclosing whole minute.
    let expiry = expiry.strftime("%Y-%m-%dT%H:%M:%SZ").to_string();

    let output = az_command()
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

/// Build the [`std::process::Command`] used to invoke the Azure CLI.
///
/// On Windows the Azure CLI is a `az.cmd` batch shim; `std::process` does not
/// honor `PATHEXT`, so a bare `az` fails to resolve it. Going through the command
/// interpreter (`cmd /C az ...`) lets Windows apply `PATHEXT` and find the shim.
#[cfg(all(feature = "clap", windows))]
fn az_command() -> std::process::Command {
    let mut command = std::process::Command::new("cmd");
    command.args(["/C", "az"]);
    command
}

#[cfg(all(feature = "clap", not(windows)))]
fn az_command() -> std::process::Command {
    std::process::Command::new("az")
}
