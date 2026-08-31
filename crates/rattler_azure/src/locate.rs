use crate::{AzureChannelUrl, AzureEndpointKey, AzureUrlError, ContainerName};

/// A channel URL bundled with the endpoint key and container derived from it
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AzureLocation {
    channel: AzureChannelUrl,
    key: Option<AzureEndpointKey>,
    container: Option<ContainerName>,
}

impl AzureLocation {
    pub fn channel(&self) -> &AzureChannelUrl {
        &self.channel
    }

    pub fn key(&self) -> Option<&AzureEndpointKey> {
        self.key.as_ref()
    }

    pub fn container(&self) -> Option<&ContainerName> {
        self.container.as_ref()
    }

    /// The key and container, or an error naming which is missing
    pub fn addressed(&self) -> Result<(&AzureEndpointKey, &ContainerName), AzureUrlError> {
        let key = self
            .key
            .as_ref()
            .ok_or_else(|| AzureUrlError::InvalidHost(self.channel.host().to_string()))?;
        Ok((
            key,
            self.container.as_ref().ok_or(AzureUrlError::NoContainer)?,
        ))
    }
}

/// Match a channel URL against the configured entry keys.
///
/// Both of the URL's candidates are tried, longest first, so
/// `proxy.internal/accta` wins over `proxy.internal` where both are configured.
/// A URL matching neither is read host-style, the shape of the default entry.
///
/// # Arguments
///
/// * `channel` - the URL to locate
/// * `configured` - whether a candidate key has an entry
pub fn locate(
    channel: &AzureChannelUrl,
    configured: impl Fn(&AzureEndpointKey) -> bool,
) -> Result<AzureLocation, AzureUrlError> {
    let host_style = AzureEndpointKey::host_style(channel.host()).ok();
    let path_style = segment(channel, 0)
        .and_then(|segment| AzureEndpointKey::path_style(channel.host().clone(), segment).ok());

    let key = [path_style, host_style.clone()]
        .into_iter()
        .flatten()
        .find(|key| configured(key))
        .or(host_style);

    located(channel, key)
}

/// Locate a channel under the addressing the caller states outright, with no
/// key matching
pub fn locate_as(
    channel: &AzureChannelUrl,
    addressing: AzureAddressing,
) -> Result<AzureLocation, AzureUrlError> {
    let key = match addressing {
        AzureAddressing::HostStyle => AzureEndpointKey::host_style(channel.host())?,
        AzureAddressing::PathStyle => AzureEndpointKey::path_style(
            channel.host().clone(),
            segment(channel, 0).unwrap_or_default(),
        )?,
    };
    located(channel, Some(key))
}

/// Bundle a channel with a resolved key and the container read under that key
fn located(
    channel: &AzureChannelUrl,
    key: Option<AzureEndpointKey>,
) -> Result<AzureLocation, AzureUrlError> {
    // No key means no segment is the container
    let container = match &key {
        Some(key) => container_after(channel, key)?,
        None => None,
    };
    Ok(AzureLocation {
        channel: channel.clone(),
        key,
        container,
    })
}

/// Where a URL names its storage account.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AzureAddressing {
    /// The account is the first host label, so the path starts at the
    /// container: `az://acct.blob.core.windows.net/container/…`.
    #[default]
    HostStyle,

    /// The account is path segment 0 and the container follows it:
    /// `az://azurite.local:10000/acct/container/…`. The shape of emulators
    /// and proxies, where one host serves many accounts.
    PathStyle,
}

impl From<bool> for AzureAddressing {
    fn from(path_style: bool) -> Self {
        if path_style {
            AzureAddressing::PathStyle
        } else {
            AzureAddressing::HostStyle
        }
    }
}

/// The container segment under the key's addressing.
///
/// `Ok(None)`: the URL stops before the container. `Err`: the segment exists
/// but is not a legal Azure container name.
fn container_after(
    channel: &AzureChannelUrl,
    key: &AzureEndpointKey,
) -> Result<Option<ContainerName>, AzureUrlError> {
    segment(channel, key.container_segment())
        .map(ContainerName::new)
        .transpose()
}

fn segment(channel: &AzureChannelUrl, index: usize) -> Option<&str> {
    // The `is_empty` filter is only sound because `AzureChannelUrl::parse` rejects
    // an empty segment anywhere but the end
    channel
        .path()
        .segments()
        .nth(index)
        .filter(|segment| !segment.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{channel, container, key};

    /// One `url configured=[keys]` entry per case; the snapshot pins the
    /// matched key and container, or the error.
    #[test]
    fn located_outcomes() {
        let cases: &[(&str, &[&str])] = &[
            // a configured or fallback host-style key grants the container
            ("az://acct.blob.core.windows.net/general/noarch", &[]),
            (
                "az://acct.blob.core.windows.net/general/noarch",
                &["acct.blob.core.windows.net"],
            ),
            // the configured key decides where the container sits
            (
                "az://proxy.internal/accta/general/noarch",
                &["proxy.internal/accta"],
            ),
            ("az://proxy.internal/accta/general", &["proxy.internal"]),
            // the longest configured key wins
            (
                "az://proxy.internal/accta/general/noarch",
                &["proxy.internal", "proxy.internal/accta"],
            ),
            (
                "az://127.0.0.1:10000/devstoreaccount1/general",
                &["127.0.0.1:10000/devstoreaccount1"],
            ),
            // an IP literal has no account label, so no host-style fallback
            ("az://127.0.0.1:10000/devstoreaccount1/general", &[]),
            // an illegal container name errors once a key grants one
            ("az://acct.blob.core.windows.net/General/noarch", &[]),
            ("az://acct.blob.core.windows.net/ab/noarch", &[]),
            ("az://acct.blob.core.windows.net/a--b/noarch", &[]),
            ("az://acct.blob.core.windows.net/general;evil/noarch", &[]),
            ("az://acct.blob.core.windows.net/-o/noarch", &[]),
            // unkeyed URLs report no container rather than a bad one
            (
                "az://127.0.0.1:10000/Conda_Channel/noarch/repodata.json",
                &[],
            ),
            ("az://azurite/Conda_Channel/noarch/repodata.json", &[]),
            ("az://mirror/ab/noarch/repodata.json", &[]),
            ("az://localhost:8080/My_Repo/noarch/repodata.json", &[]),
        ];

        let outcomes: indexmap::IndexMap<String, String> = cases
            .iter()
            .map(|(url, configured)| {
                let keys = configured.iter().copied().map(key).collect::<Vec<_>>();
                let outcome = match locate(&channel(url), |candidate| keys.contains(candidate)) {
                    Ok(located) => format!(
                        "key: {}, container: {}",
                        located.key().map_or("none".into(), ToString::to_string),
                        located
                            .container()
                            .map_or("none".into(), ToString::to_string),
                    ),
                    Err(err) => format!("error: {err}"),
                };
                (format!("{url} configured={configured:?}"), outcome)
            })
            .collect();
        insta::assert_yaml_snapshot!(outcomes);
    }

    #[test]
    fn a_path_style_key_takes_the_account_off_any_host() {
        for host in ["127.0.0.1:10000", "azurite"] {
            let key = key(&format!("{host}/devstoreaccount1"));
            assert_eq!(key.account().as_str(), "devstoreaccount1", "{host}");

            let located = locate_as(
                &channel(&format!("az://{host}/devstoreaccount1/general/noarch")),
                AzureAddressing::PathStyle,
            )
            .unwrap_or_else(|err| panic!("{host} should locate path-style: {err}"));
            assert_eq!(located.addressed().unwrap(), (&key, &container("general")));
        }
    }

    #[test]
    fn a_url_short_of_the_container_has_none_to_address() {
        for (url, addressing) in [
            (
                "az://127.0.0.1:10000/devstoreaccount1",
                AzureAddressing::PathStyle,
            ),
            (
                "az://acct.blob.core.windows.net",
                AzureAddressing::HostStyle,
            ),
            (
                "az://acct.blob.core.windows.net/",
                AzureAddressing::HostStyle,
            ),
        ] {
            let located =
                locate_as(&channel(url), addressing).unwrap_or_else(|err| panic!("{err}"));
            assert_eq!(located.container(), None, "{url}");
            assert!(
                matches!(located.addressed(), Err(AzureUrlError::NoContainer)),
                "{url}"
            );
        }

        assert!(matches!(
            locate_as(
                &channel("az://127.0.0.1:10000/"),
                AzureAddressing::PathStyle
            ),
            Err(AzureUrlError::InvalidAccountName(_))
        ));
    }
}
