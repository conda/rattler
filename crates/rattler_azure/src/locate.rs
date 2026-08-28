use crate::{AzureChannelUrl, AzureEndpointKey, AzureUrlError, ContainerName};

/// A channel URL bundled with the endpoint key and container derived from it, so
/// the three can never be mixed and matched across different endpoints.
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

    /// The key the URL matched, or the host-style key it falls back to.
    ///
    /// `None` when neither exists: an unconfigured IP literal names no account, so
    /// there is nothing for a grant to hang off.
    pub fn key(&self) -> Option<&AzureEndpointKey> {
        self.key.as_ref()
    }

    pub fn container(&self) -> Option<&ContainerName> {
        self.container.as_ref()
    }

    /// The key and container a request has to name, or the reason the URL names
    /// neither.
    ///
    /// The fetch path can send anonymously without either; anything that writes,
    /// signs or builds an endpoint needs both.
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
/// Both candidates are tried, longest first, so `proxy.internal/accta` wins over
/// `proxy.internal` where both are configured. A URL matching neither is read
/// host-style, the shape of the default entry.
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

/// Locate a channel under the addressing the caller states outright.
///
/// For a caller with no `azure-options` table to match against, where the account's
/// whereabouts is the one thing the URL cannot say: `az://proxy.internal/accta/…`
/// reads as account `proxy` or account `accta` with equal justification.
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

fn located(
    channel: &AzureChannelUrl,
    key: Option<AzureEndpointKey>,
) -> Result<AzureLocation, AzureUrlError> {
    let container = container_after(channel, key.as_ref())?;
    Ok(AzureLocation {
        channel: channel.clone(),
        key,
        container,
    })
}

/// Deliberately just the missing bit, not the account itself: the account is
/// already at path segment 0 of the URL, and a caller naming it separately could
/// contradict the URL and sign for a different account than the one addressed.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AzureAddressing {
    #[default]
    HostStyle,

    PathStyle,
}

impl From<bool> for AzureAddressing {
    fn from(path_style: bool) -> Self {
        if path_style {
            Self::PathStyle
        } else {
            Self::HostStyle
        }
    }
}

/// `Ok(None)` means there is nothing to attribute a grant to: the URL has no
/// container segment, or no key, in which case no segment is the container. `Err`
/// means the segment is there but is not a name Azure allows, which is a malformed
/// endpoint rather than an ungranted one — and only reportable when a key matched,
/// since that is the only case where a grant could otherwise be missed.
fn container_after(
    channel: &AzureChannelUrl,
    key: Option<&AzureEndpointKey>,
) -> Result<Option<ContainerName>, AzureUrlError> {
    let Some(key) = key else {
        return Ok(None);
    };
    segment(channel, key.container_segment())
        .map(ContainerName::new)
        .transpose()
}

fn segment(channel: &AzureChannelUrl, index: usize) -> Option<&str> {
    // The `is_empty` filter is only sound because `AzureChannelUrl::parse` rejects
    // an empty segment anywhere but the end: the sole one that can reach here is a
    // trailing slash, where "absent" is the right reading. Without that guarantee
    // `az://host//general` would read as having no container and silently fetch a
    // granted one anonymously.
    channel
        .path()
        .segments()
        .nth(index)
        .filter(|segment| !segment.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{channel, container, key, located};

    #[test]
    fn the_container_follows_the_matched_key() {
        for (url, configured) in [
            ("az://acct.blob.core.windows.net/general/noarch", &[][..]),
            (
                "az://acct.blob.core.windows.net/general/noarch",
                &["acct.blob.core.windows.net"],
            ),
            (
                "az://proxy.internal/accta/general/noarch",
                &["proxy.internal/accta"],
            ),
            ("az://proxy.internal/accta/general", &["proxy.internal"]),
            (
                "az://127.0.0.1:10000/devstoreaccount1/general",
                &["127.0.0.1:10000/devstoreaccount1"],
            ),
        ] {
            let located = located(url, configured);
            let Some(container) = located.container() else {
                panic!("{url} names a container");
            };
            let prefix = match located.key() {
                Some(AzureEndpointKey::PathStyle(path)) => format!("/{}", path.account()),
                Some(AzureEndpointKey::HostStyle(_)) | None => String::new(),
            };
            assert!(
                channel(url)
                    .canonical()
                    .path()
                    .starts_with(&format!("{prefix}/{container}")),
                "{url}: `{container}` does not follow `{prefix}`"
            );
        }
    }

    #[test]
    fn the_longest_configured_key_wins() {
        let url = "az://proxy.internal/accta/general/noarch";

        let both = located(url, &["proxy.internal", "proxy.internal/accta"]);
        assert_eq!(both.key(), Some(&key("proxy.internal/accta")));
        assert_eq!(both.container(), Some(&container("general")));

        let host_only = located(url, &["proxy.internal"]);
        assert_eq!(host_only.key(), Some(&key("proxy.internal")));
        assert_eq!(host_only.container(), Some(&container("accta")));
    }

    /// An unconfigured IP literal is read host-style, which names no account — so
    /// it has no key, and nothing a grant could hang off.
    #[test]
    fn an_unmatched_url_falls_back_to_host_style() {
        let anonymous = located("az://127.0.0.1:10000/devstoreaccount1/general", &[]);
        assert_eq!(anonymous.key(), None);
        assert_eq!(anonymous.container(), None);

        let azure = located("az://acct.blob.core.windows.net/general/noarch", &[]);
        assert_eq!(azure.key(), Some(&key("acct.blob.core.windows.net")));
        assert_eq!(azure.container(), Some(&container("general")));
    }

    #[test]
    fn a_url_without_a_container_names_none() {
        for (url, configured) in [
            ("az://acct.blob.core.windows.net", &[][..]),
            ("az://acct.blob.core.windows.net/", &[]),
            (
                "az://127.0.0.1:10000/devstoreaccount1",
                &["127.0.0.1:10000/devstoreaccount1"],
            ),
            ("az://127.0.0.1:10000/", &[]),
        ] {
            assert_eq!(located(url, configured).container(), None, "{url}");
        }
    }

    #[test]
    fn a_url_with_an_unusable_container_is_an_error() {
        for url in [
            "az://acct.blob.core.windows.net/General/noarch",
            "az://acct.blob.core.windows.net/ab/noarch",
            "az://acct.blob.core.windows.net/a--b/noarch",
            "az://acct.blob.core.windows.net/general;evil/noarch",
            "az://acct.blob.core.windows.net/-o/noarch",
        ] {
            let err = locate(&channel(url), |_| false)
                .expect_err("an illegal container name must be reported");
            assert!(
                matches!(err, AzureUrlError::InvalidContainerName(_)),
                "{url}: {err}"
            );
        }
    }

    /// Without a key nothing is granted, so a segment Azure would refuse as a
    /// container is not this parse's business: refusing it turns an anonymous fetch
    /// a user had working into a hard error.
    #[test]
    fn an_unkeyed_url_reports_no_container_rather_than_a_bad_one() {
        for url in [
            "az://127.0.0.1:10000/Conda_Channel/noarch/repodata.json",
            "az://azurite/Conda_Channel/noarch/repodata.json",
            "az://mirror/ab/noarch/repodata.json",
            "az://localhost:8080/My_Repo/noarch/repodata.json",
        ] {
            let located = located(url, &[]);
            assert_eq!(located.key(), None, "{url}");
            assert_eq!(located.container(), None, "{url}");
        }
    }

    #[test]
    fn a_path_style_key_takes_the_account_off_any_host() {
        for host in [
            "127.0.0.1:10000",
            "[::1]:10000",
            "azurite:10000",
            "localhost:10000",
            "azurite",
            "localhost",
        ] {
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
