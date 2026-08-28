use secrecy::ExposeSecret;

use crate::{AzureCredentials, AzureLocation, AzureScheme, AzureUrlError};

/// opendal's azblob core builds every request URI as `{endpoint}/{container}/{path}`
/// and carries no account field, so under a path-style key the account can only
/// reach the URL through `endpoint` — which is exactly what the key spells, under
/// both shapes. `root` is the channel path past the key and the container.
///
/// `account_name` is mandatory under both shapes: opendal infers it only from
/// three known Azure suffixes and returns `None` rather than an error otherwise,
/// so omitting it makes shared-key signing quietly never engage and surfaces as a
/// 403.
///
/// The endpoint never ends in a slash. `AzblobBuilder::endpoint` trims one, but
/// this builds the config struct literally, where nothing does.
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

    /// Asserted string by string, because every field but `container` differs from
    /// host-style and each fails silently when wrong: a missing `account_name` is a
    /// 403, a stray slash gives `//container/…`, a short `root` writes the channel
    /// one directory too deep.
    #[test]
    fn azblob_config_under_path_style() {
        let config = azblob_config(
            &AzureCredentials::AccountKey("key".into()),
            &path_style("az://127.0.0.1:10000/devstoreaccount1/general/mychannel"),
            AzureScheme::Http,
        )
        .unwrap();

        assert_eq!(
            config.endpoint.as_deref(),
            Some("http://127.0.0.1:10000/devstoreaccount1")
        );
        assert_eq!(config.account_name.as_deref(), Some("devstoreaccount1"));
        assert_eq!(config.container, "general");
        assert_eq!(config.root.as_deref(), Some("/mychannel"));
        assert_eq!(config.account_key.as_deref(), Some("key"));

        let endpoint = config.endpoint.unwrap();
        assert!(!endpoint.ends_with('/'), "{endpoint}");
        let root = config.root.unwrap();
        assert!(
            !root.contains("general"),
            "the container must not appear in the root: {root}"
        );
        assert!(
            !root.contains("devstoreaccount1"),
            "the account must not appear in the root: {root}"
        );
    }

    /// A bare `account/container` leaves nothing for the root, which must still be
    /// `/`, not the empty string opendal treats as a relative path.
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
    fn azblob_config_under_host_style_is_unchanged() {
        let config = azblob_config(
            &AzureCredentials::SasToken("sv=token".into()),
            &located(
                "az://stcondachannel.blob.core.windows.net/general/sub/dir",
                &[],
            ),
            AzureScheme::Https,
        )
        .unwrap();

        assert_eq!(
            config.endpoint.as_deref(),
            Some("https://stcondachannel.blob.core.windows.net")
        );
        assert_eq!(config.account_name.as_deref(), Some("stcondachannel"));
        assert_eq!(config.container, "general");
        assert_eq!(config.root.as_deref(), Some("/sub/dir"));
        assert_eq!(config.sas_token.as_deref(), Some("sv=token"));
        assert_eq!(config.account_key, None);
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
