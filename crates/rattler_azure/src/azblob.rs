use secrecy::ExposeSecret;

use crate::{AzureCredentials, AzureLocation, AzureScheme, AzureUrlError};

/// Build an [`opendal::services::AzblobConfig`] for a given azure blob.
pub fn azblob_config(
    credentials: &AzureCredentials,
    location: &AzureLocation,
    scheme: AzureScheme,
) -> Result<opendal::services::AzblobConfig, AzureUrlError> {
    let (key, container) = location.addressed()?;
    let endpoint = format!("{scheme}://{key}");

    // opendal percent-encodes `root + path` again, so the wire form would
    // double-encode a prefix containing a space or a `+`. `addressed` has already
    // confirmed the consumed segments exist.
    let root = format!(
        "/{}",
        location
            .channel()
            .path()
            .decoded_segments()
            .skip(key.segments_before_root())
            .collect::<Vec<_>>()
            .join("/")
    );

    let (account_key, sas_token) = match credentials {
        AzureCredentials::AccountKey(key) => (Some(key.expose_secret().to_string()), None),
        AzureCredentials::SasToken(token) => {
            let token = token.expose_secret();
            (
                None,
                Some(token.strip_prefix('?').unwrap_or(token).to_string()),
            )
        }
    };

    Ok(opendal::services::AzblobConfig {
        endpoint: Some(endpoint),
        account_name: Some(key.account().as_str().to_string()),
        container: container.as_str().to_string(),
        root: Some(root),
        account_key,
        sas_token,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{located, path_style};

    #[test]
    fn azblob_config_path_style_without_a_prefix() {
        let config = azblob_config(
            &AzureCredentials::SasToken("?sv=token".into()),
            &path_style("az://127.0.0.1:10000/devstoreaccount1/general"),
            AzureScheme::Http,
        )
        .unwrap();

        assert_eq!(config.root.as_deref(), Some("/"));
        assert_eq!(config.container, "general");
        assert_eq!(config.sas_token.as_deref(), Some("sv=token"));
    }

    #[test]
    fn azblob_config_under_host_style() {
        for scheme in [AzureScheme::Https, AzureScheme::Http] {
            let config = azblob_config(
                &AzureCredentials::SasToken("sv=token".into()),
                &located(
                    "az://stcondachannel.blob.core.windows.net/general/sub/dir",
                    &[],
                ),
                scheme,
            )
            .unwrap();

            assert_eq!(
                config.endpoint.as_deref(),
                Some(
                    format!("{}://stcondachannel.blob.core.windows.net", scheme.as_str()).as_str()
                )
            );
            assert_eq!(config.account_name.as_deref(), Some("stcondachannel"));
            assert_eq!(config.container, "general");
            assert_eq!(config.root.as_deref(), Some("/sub/dir"));
            assert_eq!(config.sas_token.as_deref(), Some("sv=token"));
            assert_eq!(config.account_key, None);
        }
    }

    #[test]
    fn azblob_config_decodes_the_root() {
        let config = azblob_config(
            &AzureCredentials::AccountKey("key".into()),
            &located("az://acct.blob.core.windows.net/general/with%20space", &[]),
            AzureScheme::Https,
        )
        .unwrap();

        assert_eq!(config.root.as_deref(), Some("/with space"));
    }
}
