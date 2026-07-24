//! Helpers for deriving Azure Blob coordinates from channel URLs and for minting
//! short-lived credentials for them.
//!
//! # Trusted-host model
//!
//! [`account_and_container`] trusts the URL host verbatim: whatever host is
//! named is taken to be the storage endpoint, and any ambient AAD credentials
//! (an `az login` session) are sent to that host. Userinfo (`user:pass@host`) is
//! rejected because it is a host-spoofing vector, but an honest, arbitrary host
//! is the caller's responsibility — this crate does not police which hosts are
//! legitimate Azure endpoints.

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
///
/// The type deliberately has no `Serialize`/`Deserialize`: it holds raw account
/// keys and SAS tokens, so serialization would risk leaking secrets to disk. For
/// the same reason `Debug` is implemented by hand to redact the secret values
/// rather than derived.
#[derive(Clone)]
pub enum AzureCredentials {
    /// A shared storage account key.
    AccountKey(String),

    /// A shared access signature (SAS) token.
    SasToken(String),
}

impl std::fmt::Debug for AzureCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Print only the variant, never the secret it carries.
        let variant = match self {
            AzureCredentials::AccountKey(_) => "AccountKey",
            AzureCredentials::SasToken(_) => "SasToken",
        };
        f.debug_tuple(variant).field(&"<redacted>").finish()
    }
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

    /// The URL carries userinfo (`user:pass@host`).
    #[error(
        "Azure blob URL must not contain userinfo (`user:pass@host`): the `user@host` form is a \
         host-spoofing vector that can disguise the real target host, and userinfo is invalid in \
         blob URLs"
    )]
    UserInfoNotAllowed,

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

    /// The derived account or container contains characters outside `[a-z0-9-]`.
    #[error(
        "Azure blob URL component `{0}` contains characters outside [a-z0-9-]; account and \
         container names are restricted to that set"
    )]
    InvalidCharacters(String),

    /// The channel URL string could not be parsed.
    #[error("`{value}` is not a valid URL")]
    InvalidUrl {
        /// The offending input.
        value: String,
        /// The underlying parse error.
        #[source]
        source: url::ParseError,
    },

    /// The channel URL does not use the `az://` scheme.
    #[error(
        "Azure blob channel URL must use the `az://` scheme, e.g. \
         `az://<account>.blob.core.windows.net/<container>/...`: got `{0}`"
    )]
    InvalidScheme(String),
}

/// The storage account and container an Azure Blob channel URL resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureCoordinates {
    /// The storage account name (first label of the host).
    pub account: String,

    /// The blob container name (first path segment).
    pub container: String,
}

/// Derive the storage account name and container from an Azure Blob channel URL
/// of the form `https://<account>.blob.core.windows.net/<container>/<prefix>`.
///
/// The account name is the first label of the host, so the host must be a dotted
/// domain; IP-literal and single-label hosts (e.g. `localhost` or the Azurite
/// emulator) are rejected because no account can be derived from them.
///
/// The host is otherwise trusted verbatim (see the [crate-level docs] for the
/// trusted-host model): userinfo (`user:pass@host`) is rejected as a
/// host-spoofing vector, but an honest, arbitrary host is the caller's
/// responsibility. The derived account and container are additionally restricted
/// to `[a-z0-9-]` so that argument-injection-shaped values can never reach the
/// `az` subprocess.
///
/// [crate-level docs]: crate
pub fn account_and_container(url: &Url) -> Result<AzureCoordinates, AzureUrlError> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AzureUrlError::UserInfoNotAllowed);
    }
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

    for component in [account, container] {
        if !is_valid_component(component) {
            return Err(AzureUrlError::InvalidCharacters(component.to_string()));
        }
    }

    Ok(AzureCoordinates {
        account: account.to_string(),
        container: container.to_string(),
    })
}

fn is_valid_component(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Parse and validate an Azure Blob **channel** URL.
///
/// The only accepted form is the `az://` channel scheme —
/// `az://<account>.blob.<suffix>/<container>/<prefix>` — which is rewritten to
/// `https://` here so every consumer downstream works with a real wire URL and
/// never sees the `az` scheme. A bare `http(s)://` URL is deliberately *not*
/// accepted: `az://` is the single canonical spelling for an Azure channel
/// (matching how it is written in configuration and used on the fetch path), and
/// accepting the wire URL as a second spelling would only invite confusion. The
/// host and container are validated via [`account_and_container`].
pub fn parse_channel_url(value: &str) -> Result<Url, AzureUrlError> {
    // Require the `az://` scheme, then rewrite to `https://` before parsing so
    // the host is parsed by the URL crate's special-scheme host parser and the
    // `az` scheme never leaks downstream.
    let rest = value
        .strip_prefix("az://")
        .ok_or_else(|| AzureUrlError::InvalidScheme(value.to_string()))?;
    let url =
        Url::parse(&format!("https://{rest}")).map_err(|source| AzureUrlError::InvalidUrl {
            value: value.to_string(),
            source,
        })?;
    account_and_container(&url)?;
    Ok(url)
}

/// Build an opendal [`AzblobConfig`](opendal::services::AzblobConfig) from a
/// channel URL and credentials.
///
/// The account name, endpoint, container, and root prefix are all derived from
/// the URL (`https://<account>.blob.core.windows.net/<container>/<prefix>`); the
/// credentials supply only the account key or SAS token. The URL is expected to
/// already be validated and normalized to `https://` (see [`parse_channel_url`]).
#[cfg(feature = "opendal")]
pub fn azblob_config(
    credentials: &AzureCredentials,
    channel: &Url,
) -> Result<opendal::services::AzblobConfig, AzureUrlError> {
    let AzureCoordinates { account, container } = account_and_container(channel)?;

    // Preserve a non-default port if one is present; real Azure uses the scheme
    // default (443).
    let host = channel.host_str().ok_or(AzureUrlError::NoHost)?;
    let authority = match channel.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };

    // Root prefix = the path after the container segment. Percent-decode each
    // segment: `path_segments()` yields still-encoded segments, and opendal
    // percent-encodes the root again, so passing them through verbatim would
    // double-encode prefixes containing spaces or `+`. `account_and_container`
    // has already confirmed there is at least the container segment.
    let root = format!(
        "/{}",
        channel
            .path_segments()
            .into_iter()
            .flatten()
            .skip(1)
            .map(|segment| percent_encoding::percent_decode_str(segment).decode_utf8_lossy())
            .collect::<Vec<_>>()
            .join("/")
    );

    let (account_key, sas_token) = match credentials {
        AzureCredentials::AccountKey(key) => (Some(key.clone()), None),
        AzureCredentials::SasToken(token) => (None, Some(normalize_sas_token(token).to_string())),
    };

    Ok(opendal::services::AzblobConfig {
        endpoint: Some(format!("{}://{}", channel.scheme(), authority)),
        account_name: Some(account),
        container,
        root: Some(root),
        account_key,
        sas_token,
        ..Default::default()
    })
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

    /// The `az` executable could not be resolved on `PATH`.
    #[error("could not resolve the Azure CLI (`az`) on PATH; install it and run `az login`")]
    AzResolve(#[source] which::Error),

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
/// Runs the `az` process on the tokio runtime; it is meant to be called once at
/// setup time.
///
/// # Container-scope limitation
///
/// A user-delegation SAS minted against a flat container is *container-scoped*,
/// not prefix-scoped: it grants its permissions over the whole container, so a
/// SAS for one channel also grants rights over any sibling channels that share
/// the same container. The short TTL requested here bounds the blast radius, but
/// prefix-scoping a flat container is not possible without a stored access
/// policy, which this path deliberately does not create.
#[cfg(feature = "clap")]
pub async fn mint_user_delegation_sas(
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

    let mut command = az_command()?;

    let output = command
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
        .await
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

/// Build the [`tokio::process::Command`] used to invoke the Azure CLI.
///
/// `which` resolves `az` up front so a missing CLI surfaces as [`AzureCliSasError::AzResolve`]
/// rather than an opaque spawn failure. It also matters on Windows, where the CLI
/// is an `az.cmd` batch shim: the process spawner does not honor `PATHEXT`, so a
/// bare `az` fails to resolve, but `which` applies `PATHEXT` to find the real path.
/// The resolved path is invoked directly; routing through the command interpreter
/// (`cmd /C az ...`) is deliberately avoided as an argument-injection vector.
#[cfg(feature = "clap")]
fn az_command() -> Result<tokio::process::Command, AzureCliSasError> {
    let path = which::which("az").map_err(AzureCliSasError::AzResolve)?;
    Ok(tokio::process::Command::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_url_resolves() {
        let url = Url::parse("https://acct.blob.core.windows.net/general/noarch").unwrap();
        assert_eq!(
            account_and_container(&url).unwrap(),
            AzureCoordinates {
                account: "acct".to_string(),
                container: "general".to_string(),
            }
        );
    }

    #[test]
    fn userinfo_is_rejected() {
        let url = Url::parse("https://acct.blob.core.windows.net@evil.example/general").unwrap();
        assert!(matches!(
            account_and_container(&url),
            Err(AzureUrlError::UserInfoNotAllowed)
        ));
    }

    #[test]
    fn invalid_charset_container_is_rejected() {
        let url = Url::parse("https://acct.blob.core.windows.net/general;evil/noarch").unwrap();
        assert!(matches!(
            account_and_container(&url),
            Err(AzureUrlError::InvalidCharacters(_))
        ));
    }

    #[test]
    fn parse_channel_url_normalizes_az_to_https() {
        let url = parse_channel_url("az://acct.blob.core.windows.net/general/noarch").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(
            url.as_str(),
            "https://acct.blob.core.windows.net/general/noarch"
        );
    }

    #[test]
    fn parse_channel_url_rejects_bare_http_and_https() {
        for input in [
            "https://acct.blob.core.windows.net/general",
            "http://acct.blob.core.windows.net/general",
            "ftp://acct.blob.core.windows.net/general",
            "AZ://acct.blob.core.windows.net/general",
            "acct.blob.core.windows.net/general",
        ] {
            assert!(
                matches!(
                    parse_channel_url(input),
                    Err(AzureUrlError::InvalidScheme(_))
                ),
                "expected InvalidScheme for {input}"
            );
        }
    }

    #[test]
    fn parse_channel_url_propagates_validation_errors() {
        assert!(matches!(
            parse_channel_url("az://acct.blob.core.windows.net@evil.example/general"),
            Err(AzureUrlError::UserInfoNotAllowed)
        ));
    }
}

#[cfg(test)]
mod debug_redaction_tests {
    use super::*;

    #[test]
    fn debug_never_prints_secret() {
        for creds in [
            AzureCredentials::AccountKey("supersecretkey".into()),
            AzureCredentials::SasToken("sig=deadbeef".into()),
        ] {
            let out = format!("{creds:?}");
            assert!(out.contains("<redacted>"), "not redacted: {out}");
            assert!(!out.contains("supersecret"));
            assert!(!out.contains("deadbeef"));
        }
    }
}
