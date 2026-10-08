use crate::{AzureChannelUrl, AzureEndpointKey, AzureUrlError, ContainerName};

/// Where a channel URL's endpoint key came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    Configured,
    Derived,
}

/// A channel URL resolved to an endpoint key.
#[derive(Debug)]
pub enum AzureLocation {
    Keyed(KeyedLocation),
    Unkeyed {
        channel: AzureChannelUrl,
        reason: AzureUrlError,
    },
}

impl AzureLocation {
    pub fn channel(&self) -> &AzureChannelUrl {
        match self {
            Self::Keyed(keyed) => keyed.channel(),
            Self::Unkeyed { channel, .. } => channel,
        }
    }
}

/// A channel URL with the endpoint key it is addressed by and the container
/// that follows the key, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyedLocation {
    channel: AzureChannelUrl,
    key: AzureEndpointKey,
    source: KeySource,
    container: Option<ContainerName>,
}

impl KeyedLocation {
    pub fn channel(&self) -> &AzureChannelUrl {
        &self.channel
    }

    pub fn key(&self) -> &AzureEndpointKey {
        &self.key
    }

    pub fn source(&self) -> KeySource {
        self.source
    }

    pub fn container(&self) -> Option<&ContainerName> {
        self.container.as_ref()
    }
}

/// Resolve a channel URL to an endpoint key.
///
/// A configured path-style key is preferred over a configured host-style one.
/// A URL matching neither gets a derived host-style key if its host is a known
/// Azure blob endpoint, and is otherwise unkeyed.
pub fn locate(
    channel: &AzureChannelUrl,
    is_configured: impl Fn(&AzureEndpointKey) -> bool,
) -> Result<AzureLocation, AzureUrlError> {
    let path_style = segment(channel, 0)
        .and_then(|segment| AzureEndpointKey::path_style(channel.host().clone(), segment).ok());
    let host_style = AzureEndpointKey::host_style(channel.host()).ok();

    let matched = [path_style, host_style]
        .into_iter()
        .flatten()
        .find(|key| is_configured(key));

    let (key, source) = match matched {
        Some(key) => (key, KeySource::Configured),
        None => match derive(channel) {
            Ok(key) => (key, KeySource::Derived),
            Err(reason) => {
                let reason = miscased_account(channel, &is_configured).unwrap_or(reason);
                return Ok(AzureLocation::Unkeyed {
                    channel: channel.clone(),
                    reason,
                });
            }
        },
    };

    let container = segment(channel, key.path_segments())
        .map(ContainerName::from_segment)
        .transpose()?;

    Ok(AzureLocation::Keyed(KeyedLocation {
        channel: channel.clone(),
        key,
        source,
        container,
    }))
}

fn derive(channel: &AzureChannelUrl) -> Result<AzureEndpointKey, AzureUrlError> {
    if channel.host().is_known_azure_blob_endpoint() {
        AzureEndpointKey::host_style(channel.host())
    } else if AzureEndpointKey::host_style(channel.host()).is_ok() {
        Err(AzureUrlError::UnconfiguredHost(channel.host().to_string()))
    } else {
        Err(AzureUrlError::UnconfiguredHostWithoutAccount(
            channel.host().to_string(),
        ))
    }
}

/// The account segment when only its case keeps it from a configured
/// path-style key.
fn miscased_account(
    channel: &AzureChannelUrl,
    is_configured: impl Fn(&AzureEndpointKey) -> bool,
) -> Option<AzureUrlError> {
    let written = segment(channel, 0)?;
    let lowercased =
        AzureEndpointKey::path_style(channel.host().clone(), &written.to_ascii_lowercase()).ok()?;
    is_configured(&lowercased).then(|| AzureUrlError::InvalidAccountName(written.to_string()))
}

fn segment(channel: &AzureChannelUrl, index: usize) -> Option<&str> {
    channel
        .path()
        .segments()
        .nth(index)
        .filter(|segment| !segment.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{channel, key};

    fn describe(location: &AzureLocation) -> String {
        match location {
            AzureLocation::Keyed(keyed) => {
                let container = keyed
                    .container()
                    .map_or("none".to_string(), ToString::to_string);
                format!(
                    "keyed {:?}: {}, container: {container}",
                    keyed.source(),
                    keyed.key(),
                )
            }
            AzureLocation::Unkeyed { reason, .. } => format!("unkeyed: {reason}"),
        }
    }

    #[test]
    fn located_urls() {
        let cases: &[(&str, &[&str])] = &[
            ("az://acct.blob.core.windows.net/general/noarch", &[]),
            (
                "az://acct.blob.core.windows.net/general/noarch",
                &["acct.blob.core.windows.net"],
            ),
            ("az://acct.blob.core.windows.net/gen%65ral/noarch", &[]),
            ("az://acct.blob.core.windows.net", &[]),
            ("az://acct.blob.core.windows.net/", &[]),
            ("az://acct.blob.core.windows.net/General/noarch", &[]),
            ("az://acct.blob.core.windows.net/a--b/noarch", &[]),
            ("az://acct.blob.core.windows.net:443/general/noarch", &[]),
            (
                "az://acct.blob.core.windows.net:443/general/noarch",
                &["acct.blob.core.windows.net"],
            ),
            ("az://acct-1.blob.core.windows.net/general", &[]),
            ("az://evil.acct.blob.core.windows.net/general", &[]),
            ("az://acct-1.blob.core.windows.net/General", &[]),
            (
                "az://proxy.internal/accta/general/noarch",
                &["proxy.internal/accta"],
            ),
            (
                "az://proxy.internal/accta/general/noarch",
                &["proxy.internal", "proxy.internal/accta"],
            ),
            ("az://proxy.internal/accta/general", &["proxy.internal"]),
            ("az://proxy.internal/general/noarch", &["proxy.internal"]),
            (
                "az://proxy.internal/acc%74/gen%65ral",
                &["proxy.internal/acct"],
            ),
            (
                "az://proxy.internal/ACCTA/general",
                &["proxy.internal/accta"],
            ),
            (
                "az://127.0.0.1:10000/devstoreaccount1/general",
                &["127.0.0.1:10000/devstoreaccount1"],
            ),
            (
                "az://127.0.0.1:10000/devstoreaccount1",
                &["127.0.0.1:10000/devstoreaccount1"],
            ),
            (
                "az://127.0.0.1:10000/devstoreaccount1/General",
                &["127.0.0.1:10000/devstoreaccount1"],
            ),
            ("az://127.0.0.1:10000/devstoreaccount1/general", &[]),
            ("az://mirror.example.com/Channel/noarch", &[]),
            ("az://my-mirror.example.com/general/noarch", &[]),
            ("az://azurite/Conda_Channel/noarch/repodata.json", &[]),
            (
                "az://mirror.internal:443/general/noarch",
                &["mirror.internal"],
            ),
            (
                "az://mirror.internal/general/noarch",
                &["mirror.internal:443"],
            ),
        ];

        let located: indexmap::IndexMap<String, String> = cases
            .iter()
            .map(|(url, configured)| {
                let keys = configured.iter().copied().map(key).collect::<Vec<_>>();
                let outcome = match locate(&channel(url), |candidate| keys.contains(candidate)) {
                    Ok(location) => describe(&location),
                    Err(err) => format!("error: {err}"),
                };
                (
                    format!("{url} configured [{}]", configured.join(", ")),
                    outcome,
                )
            })
            .collect();
        insta::assert_yaml_snapshot!(located);
    }
}
