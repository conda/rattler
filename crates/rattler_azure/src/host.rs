use url::Url;

use crate::AzureUrlError;

#[derive(Clone, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(try_from = "String", into = "String")
)]
pub struct AzureHost {
    host: url::Host,
    port: Option<u16>,
}

impl AzureHost {
    pub fn parse(authority: &str) -> Result<Self, AzureUrlError> {
        if authority.contains('@') {
            return Err(AzureUrlError::UserInfoNotAllowed);
        }
        if authority.contains(['/', '\\', '?', '#']) {
            return Err(AzureUrlError::InvalidHostAuthority {
                authority: authority.to_string(),
                reason: "it carries a path, query or fragment".to_string(),
            });
        }

        // parse as https to properly handle hosts but this drops `:443`, so parse as `az` for port
        let normalized = Self::parse_as(authority, "https")?;
        let verbatim = Self::parse_as(authority, "az")?;

        let port_reason = match (Self::written_port(authority), verbatim.port()) {
            (Some(""), _) => Some("its port is empty"),
            (_, Some(0)) => Some("port 0 cannot be connected to"),
            _ => None,
        };

        if let Some(reason) = port_reason {
            return Err(AzureUrlError::InvalidHostAuthority {
                authority: authority.to_string(),
                reason: reason.to_string(),
            });
        }

        let host = normalized.host().ok_or(AzureUrlError::NoHost)?.to_owned();
        Self::normalized(host, verbatim.port(), authority)
    }

    /// The text after the authority's last `:`, unless that text is an IPv6 literal
    fn written_port(authority: &str) -> Option<&str> {
        let (_, port) = authority.rsplit_once(':')?;
        (!port.ends_with(']')).then_some(port)
    }

    fn parse_as(authority: &str, scheme: &str) -> Result<Url, AzureUrlError> {
        Url::parse(&format!("{scheme}://{authority}")).map_err(|err| {
            AzureUrlError::InvalidHostAuthority {
                authority: authority.to_string(),
                reason: err.to_string(),
            }
        })
    }

    fn normalized(
        host: url::Host,
        port: Option<u16>,
        authority: &str,
    ) -> Result<Self, AzureUrlError> {
        const DNS_NAME_LIMIT: usize = 253;

        let url::Host::Domain(domain) = &host else {
            return Ok(Self { host, port });
        };

        let host = url::Host::parse(domain.strip_suffix('.').unwrap_or(domain)).map_err(|err| {
            AzureUrlError::InvalidHostAuthority {
                authority: authority.to_string(),
                reason: err.to_string(),
            }
        })?;

        if let url::Host::Domain(domain) = &host {
            if domain.split('.').any(str::is_empty) {
                return Err(AzureUrlError::InvalidHostAuthority {
                    authority: authority.to_string(),
                    reason: "one of its labels is empty".to_string(),
                });
            }

            if domain.len() > DNS_NAME_LIMIT {
                return Err(AzureUrlError::InvalidHostAuthority {
                    authority: authority.to_string(),
                    reason: format!(
                        "it is {} characters long, over the {DNS_NAME_LIMIT}-character limit DNS \
                         puts on a name",
                        domain.len()
                    ),
                });
            }
        }
        Ok(Self { host, port })
    }

    pub fn host(&self) -> &url::Host {
        &self.host
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// Whether the host is one of Azure's own blob endpoints.
    ///
    /// A proxy or private endpoint in front of real Azure returns `false`,
    /// so `false` does not mean the host is not Azure.
    pub fn is_known_azure_blob_endpoint(&self) -> bool {
        // `blob.` + each cloud's `StorageEndpointSuffix`:
        // https://learn.microsoft.com/en-us/azure/storage/common/storage-powershell-independent-clouds#endpoint-suffix
        // The German cloud's `core.cloudapi.de` is left out because that cloud closed in 2021:
        // https://learn.microsoft.com/en-us/previous-versions/azure/germany/germany-welcome
        const SUFFIXES: &[&str] = &[
            "blob.core.windows.net",
            "blob.core.usgovcloudapi.net",
            "blob.core.chinacloudapi.cn",
        ];

        let url::Host::Domain(domain) = &self.host else {
            return false;
        };

        SUFFIXES.iter().any(|suffix| {
            domain
                .strip_suffix(suffix)
                // otherwise we could match `notblob.core.windows.net`.
                .is_some_and(|prefix| prefix.ends_with('.'))
        })
    }
}

impl std::fmt::Display for AzureHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.host)?;
        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for AzureHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AzureHost({:?})", self.to_string())
    }
}

impl std::str::FromStr for AzureHost {
    type Err = AzureUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<String> for AzureHost {
    type Error = AzureUrlError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<AzureHost> for String {
    fn from(host: AzureHost) -> Self {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userinfo_is_rejected() {
        assert!(matches!(
            crate::AzureChannelUrl::parse("az://acct.blob.core.windows.net@evil.example/general"),
            Err(AzureUrlError::UserInfoNotAllowed)
        ));
        assert!(matches!(
            AzureHost::parse("acct.blob.core.windows.net@evil.example"),
            Err(AzureUrlError::UserInfoNotAllowed)
        ));
    }

    #[test]
    fn known_azure_endpoints_are_matched_on_a_label_boundary() {
        for host in [
            "acct.blob.core.windows.net",
            "acct.blob.core.usgovcloudapi.net",
            "acct.blob.core.chinacloudapi.cn",
        ] {
            assert!(
                AzureHost::parse(host)
                    .unwrap()
                    .is_known_azure_blob_endpoint(),
                "{host}"
            );
        }

        for host in [
            "notblob.core.windows.net",             // no label boundary
            "blob.core.windows.net",                // the suffix alone carries no account
            "acct.blob.core.windows.net.evil.test", // suffix in the middle
            "127.0.0.1:10000",
            "azurite",
        ] {
            assert!(
                !AzureHost::parse(host)
                    .unwrap()
                    .is_known_azure_blob_endpoint(),
                "{host}"
            );
        }
    }

    #[test]
    fn host_rejects_anything_that_is_not_a_bare_authority() {
        let inputs = [
            "acct.blob.core.windows.net/general",
            "acct.blob.core.windows.net?sv=token",
            "acct.blob.core.windows.net#frag",
            "https://acct.blob.core.windows.net",
            "",
            "acct.blob.core.windows.net:notaport",
            "acct.blob.core.windows.net:",
            "acct.blob.core.windows.net:0",
            "[::1]:",
            "[::1]:0",
            // empty labels
            "acct..blob.core.windows.net",
            "acct.blob.example..",
            ".example",
        ];

        let rejections: indexmap::IndexMap<&str, String> = inputs
            .iter()
            .map(|authority| match AzureHost::parse(authority) {
                Ok(_) => panic!("expected a rejection for {authority:?}"),
                Err(err) => (*authority, err.to_string()),
            })
            .collect();
        insta::assert_yaml_snapshot!(rejections);
    }

    #[test]
    fn host_length_is_bounded() {
        // Labels of 60, so length is the only rule under test.
        let label = "a".repeat(60);
        let too_long = format!("{}.blob.example", [label.as_str(); 8].join("."));
        assert!(AzureHost::parse(&too_long).is_err());

        // A name right at the limit still parses, so the check bounds the length
        // rather than the number of labels.
        let at_limit = format!(
            "{}.{}.blob.example",
            [label.as_str(); 3].join("."),
            "a".repeat(57)
        );
        assert_eq!(at_limit.len(), 253);
        assert!(AzureHost::parse(&at_limit).is_ok());
    }
}
