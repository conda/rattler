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

        // Two parses, each for the one thing it is authoritative about, because no
        // single scheme gives both. `https` is a special scheme, so it runs the URL
        // Standard's host parser: lowercasing, IDNA, and IP literals as typed
        // `Ipv4`/`Ipv6` hosts — but it also drops `:443`. `az` is not special, so
        // it has no default port to drop, but its opaque-host parsing leaves the
        // host unnormalized (`MyCompany.X` stays mixed case, `127.0.0.1` arrives as
        // a `Domain`). Host from the first, port from the second.
        let normalized = Self::parse_as(authority, "https")?;
        let verbatim = Self::parse_as(authority, "az")?;

        // `url` reads a bare trailing colon as "no port at all", so `host:` would
        // otherwise be accepted as `host` — a silent downgrade to a different
        // endpoint, so a dangling colon is an explicit error rather than being
        // read as "no port". Port 0 it keeps, and `wire()` then hands out
        // `https://host:0/…`, which no connection can be made to.
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

    /// The text after the authority's last `:`, unless that text ends with `]` —
    /// the closing bracket of an IPv6 literal that spells no port.
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

        // Re-run the host parser on the trimmed name so there is exactly one
        // normalization path rather than a second, hand-rolled one.
        let host = url::Host::parse(domain.strip_suffix('.').unwrap_or(domain)).map_err(|err| {
            AzureUrlError::InvalidHostAuthority {
                authority: authority.to_string(),
                reason: err.to_string(),
            }
        })?;
        if let url::Host::Domain(domain) = &host {
            // Only one trailing dot is stripped, so `acct.example..` still has an
            // empty label here — as does `acct..example`. Rejecting both is what
            // lets `Display` round-trip, and what stops account derivation from
            // handing out an empty account name.
            if domain.split('.').any(str::is_empty) {
                return Err(AzureUrlError::InvalidHostAuthority {
                    authority: authority.to_string(),
                    reason: "one of its labels is empty".to_string(),
                });
            }
            // Measured after IDNA, since the punycode form is what is resolved.
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

    /// [`parse`](Self::parse) has already rejected empty labels and a trailing
    /// dot, so those two labels are non-empty.
    pub(crate) fn account_label(&self) -> Option<&str> {
        match &self.host {
            url::Host::Domain(domain) => {
                let mut labels = domain.split('.');
                let first = labels.next()?;
                labels.next().is_some().then_some(first)
            }
            url::Host::Ipv4(_) | url::Host::Ipv6(_) => None,
        }
    }

    /// A `true` is the only evidence that a host is really Azure, which is what
    /// gates the ambient credential chain. A proxy or private endpoint in front of
    /// real Azure answers `false`, so a `false` proves nothing.
    pub fn is_known_azure_blob_endpoint(&self) -> bool {
        const SUFFIXES: &[&str] = &[
            "blob.core.windows.net",
            "blob.core.usgovcloudapi.net",
            "blob.core.chinacloudapi.cn",
        ];

        let url::Host::Domain(domain) = &self.host else {
            return false;
        };
        SUFFIXES.iter().any(|suffix| {
            // The dot has to be part of the match, or `notblob.core.windows.net`
            // would pass as `blob.core.windows.net`.
            domain
                .strip_suffix(suffix)
                .is_some_and(|prefix| prefix.ends_with('.'))
        })
    }
}

impl std::fmt::Display for AzureHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `url::Host`'s own `Display` brackets an IPv6 literal, which is what an
        // authority needs.
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
    use crate::test_support::hash_of;

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
    fn empty_host_labels_are_rejected() {
        for host in [
            "acct..blob.core.windows.net",
            "acct.blob.example..",
            ".example",
        ] {
            assert!(
                matches!(
                    AzureHost::parse(host),
                    Err(AzureUrlError::InvalidHostAuthority { .. })
                ),
                "expected a rejection for {host}"
            );
            assert!(
                matches!(
                    crate::AzureChannelUrl::parse(&format!("az://{host}/general/noarch")),
                    Err(AzureUrlError::InvalidHostAuthority { .. })
                ),
                "expected a rejection for {host}"
            );
        }
    }

    #[test]
    fn host_normalization_collapses_equivalent_spellings() {
        for (written, canonical) in [
            (
                "MyCompany.blob.core.windows.net",
                "mycompany.blob.core.windows.net",
            ),
            (
                "mycompany.blob.core.windows.net:443",
                "mycompany.blob.core.windows.net:443",
            ),
            ("ünï.blob.example", "xn--n-nga1b.blob.example"),
            ("[0:0:0:0:0:0:0:1]:10000", "[::1]:10000"),
            ("0x7f.1", "127.0.0.1"),
            ("acct.blob.core.windows.net.", "acct.blob.core.windows.net"),
        ] {
            let host = AzureHost::parse(written)
                .unwrap_or_else(|err| panic!("{written} should parse: {err}"));
            assert_eq!(host.to_string(), canonical, "{written}");

            let reparsed = AzureHost::parse(canonical).unwrap();
            assert_eq!(reparsed, host, "{written}");
            // Equal hosts must also hash equally: they key the options map.
            assert_eq!(hash_of(&host), hash_of(&reparsed), "{written}");
        }
    }

    #[test]
    fn host_equality_is_not_scheme_relative() {
        let with_port = AzureHost::parse("azurite.local:443").unwrap();
        let without = AzureHost::parse("azurite.local").unwrap();
        assert_ne!(with_port, without);
        assert_ne!(with_port, AzureHost::parse("azurite.local:80").unwrap());
        assert_eq!(with_port.to_string(), "azurite.local:443");
    }

    #[test]
    fn a_dangling_colon_is_rejected_rather_than_read_as_no_port() {
        for authority in ["acct.blob.core.windows.net:", "[::1]:"] {
            match AzureHost::parse(authority) {
                Err(AzureUrlError::InvalidHostAuthority { reason, .. }) => {
                    assert_eq!(reason, "its port is empty", "{authority}");
                }
                other => panic!("expected InvalidHostAuthority for {authority}, got {other:?}"),
            }
        }
    }

    #[test]
    fn host_rejects_anything_that_is_not_a_bare_authority() {
        // Labels of 60, so length is the only rule under test.
        let label = "a".repeat(60);
        let too_long = format!("{}.blob.example", [label.as_str(); 8].join("."));
        for authority in [
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
            &too_long,
        ] {
            assert!(
                AzureHost::parse(authority).is_err(),
                "expected a rejection for {authority:?}"
            );
        }

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
