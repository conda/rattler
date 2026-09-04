use secrecy::SecretString;

use crate::{AccountName, AzureScheme, ContainerName};

#[derive(Debug, thiserror::Error)]
pub enum AzureCliSasError {
    #[error("failed to compute the SAS expiry timestamp: {0}")]
    Expiry(String),

    #[error("could not resolve the Azure CLI (`az`) on PATH; install it and run `az login`")]
    AzResolve(#[source] which::Error),

    #[error("failed to run the Azure CLI (`az`)")]
    Spawn(#[source] std::io::Error),

    #[error("the Azure CLI failed to generate a user-delegation SAS (is `az login` current?): {0}")]
    CommandFailed(String),

    #[error("the Azure CLI returned an empty SAS token")]
    EmptyOutput,
}

/// Mint a user-delegation SAS for a container through the Azure CLI. Requires
/// `az` on `PATH` and an `az login` session.
///
/// `permissions` is the Azure SAS permission string (e.g. `"cw"`). The returned
/// token has no leading `?`, and is HTTPS-only unless `scheme` is http.
pub async fn mint_user_delegation_sas(
    account: &AccountName,
    container: &ContainerName,
    permissions: &str,
    valid_for: std::time::Duration,
    scheme: AzureScheme,
) -> Result<SecretString, AzureCliSasError> {
    let signed = jiff::SignedDuration::try_from(valid_for)
        .map_err(|err| AzureCliSasError::Expiry(err.to_string()))?;
    let expiry = jiff::Timestamp::now()
        .checked_add(signed)
        .map_err(|err| AzureCliSasError::Expiry(err.to_string()))?;
    let expiry = expiry.strftime("%Y-%m-%dT%H:%M:%SZ").to_string();

    let mut command = az_command()?;
    let output = command
        .args(generate_sas_args(
            account,
            container,
            permissions,
            &expiry,
            scheme,
        ))
        .output()
        .await
        .map_err(AzureCliSasError::Spawn)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AzureCliSasError::CommandFailed(stderr));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(AzureCliSasError::EmptyOutput);
    }
    Ok(token.into())
}

/// Generates args for the below command, without the leading `az`
/// ```bash
/// az storage container generate-sas --account-name <account> --name <container>
///     --permissions <permissions> --expiry <expiry> --auth-mode login --as-user
///     [--https-only] -o tsv
/// ```
fn generate_sas_args<'a>(
    account: &'a AccountName,
    container: &'a ContainerName,
    permissions: &'a str,
    expiry: &'a str,
    scheme: AzureScheme,
) -> Vec<&'a str> {
    let mut args = vec![
        "storage",
        "container",
        "generate-sas",
        "--account-name",
        account.as_str(),
        "--name",
        container.as_str(),
        "--permissions",
        permissions,
        "--expiry",
        expiry,
        "--auth-mode",
        "login",
        "--as-user",
    ];
    if let AzureScheme::Https = scheme {
        args.push("--https-only");
    }
    args.extend(["-o", "tsv"]);
    args
}

fn az_command() -> Result<tokio::process::Command, AzureCliSasError> {
    let path = which::which("az").map_err(AzureCliSasError::AzResolve)?;
    Ok(tokio::process::Command::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{container, key};

    #[test]
    fn https_only_follows_the_configured_scheme() {
        let key = key("acct.blob.core.windows.net");
        let container = container("general");
        let args = |scheme| {
            generate_sas_args(
                key.account(),
                &container,
                "cw",
                "2030-01-01T00:00:00Z",
                scheme,
            )
        };

        assert!(args(AzureScheme::Https).contains(&"--https-only"));
        assert!(!args(AzureScheme::Http).contains(&"--https-only"));

        for scheme in [AzureScheme::Https, AzureScheme::Http] {
            let args = args(scheme);
            assert!(args.windows(2).any(|pair| pair == ["--permissions", "cw"]));
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--expiry", "2030-01-01T00:00:00Z"])
            );
            assert!(args.contains(&"--as-user"));
            assert!(args.windows(2).any(|pair| pair == ["--auth-mode", "login"]));
        }
    }
}
