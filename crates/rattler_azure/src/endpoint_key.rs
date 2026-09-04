use crate::{AccountName, AzureChannelUrl, AzureHost, AzureUrlError};

/// A host-style azure endpoint
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountHost {
    host: AzureHost,
    account: AccountName,
}

impl AccountHost {
    pub fn new(host: AzureHost) -> Result<Self, AzureUrlError> {
        let account = AccountName::new(
            host.account_label()
                .ok_or_else(|| AzureUrlError::InvalidHost(host.to_string()))?,
        )?;
        Ok(Self { host, account })
    }

    pub fn host(&self) -> &AzureHost {
        &self.host
    }

    pub fn account(&self) -> &AccountName {
        &self.account
    }
}

impl std::fmt::Display for AccountHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.host)
    }
}

/// A path-style azure endpoint
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountPath {
    host: AzureHost,
    account: AccountName,
}

impl AccountPath {
    pub fn new(host: AzureHost, segment: &str) -> Result<Self, AzureUrlError> {
        Ok(Self {
            host,
            account: AccountName::new(segment)?,
        })
    }

    pub fn host(&self) -> &AzureHost {
        &self.host
    }

    pub fn account(&self) -> &AccountName {
        &self.account
    }
}

impl std::fmt::Display for AccountPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.host, self.account)
    }
}

/// The key of an endpoint options entry: a channel URL prefix that runs up to,
/// but not including, the container.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(try_from = "String", into = "String")
)]
pub enum AzureEndpointKey {
    HostStyle(AccountHost),
    PathStyle(AccountPath),
}

impl AzureEndpointKey {
    pub fn parse(key: &str) -> Result<Self, AzureUrlError> {
        let channel = AzureChannelUrl::parse(&format!("az://{key}"))?;
        if channel.query().is_some() || channel.fragment().is_some() {
            return Err(AzureUrlError::InvalidKey(key.to_string()));
        }

        let segments = channel
            .path()
            .segments()
            // empty segments produced by standalone or trailing `/`, any others would fail
            // the channel parse
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>();

        match segments.as_slice() {
            [] => Self::host_style(channel.host()),
            [account] => Self::path_style(channel.host().clone(), account),
            _ => Err(AzureUrlError::InvalidKey(key.to_string())),
        }
    }

    pub fn host_style(host: &AzureHost) -> Result<Self, AzureUrlError> {
        AccountHost::new(host.clone()).map(Self::HostStyle)
    }

    pub fn path_style(host: AzureHost, segment: &str) -> Result<Self, AzureUrlError> {
        AccountPath::new(host, segment).map(Self::PathStyle)
    }

    pub fn host(&self) -> &AzureHost {
        match self {
            Self::HostStyle(host) => host.host(),
            Self::PathStyle(path) => path.host(),
        }
    }

    pub fn account(&self) -> &AccountName {
        match self {
            Self::HostStyle(host) => host.account(),
            Self::PathStyle(path) => path.account(),
        }
    }

    pub(crate) fn container_segment(&self) -> usize {
        match self {
            Self::HostStyle(_) => 0,
            Self::PathStyle(_) => 1,
        }
    }

    #[cfg(feature = "opendal")]
    pub(crate) fn segments_before_root(&self) -> usize {
        self.container_segment() + 1
    }
}

impl std::fmt::Display for AzureEndpointKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostStyle(host) => write!(f, "{host}"),
            Self::PathStyle(path) => write!(f, "{path}"),
        }
    }
}

impl std::str::FromStr for AzureEndpointKey {
    type Err = AzureUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<String> for AzureEndpointKey {
    type Error = AzureUrlError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<AzureEndpointKey> for String {
    fn from(key: AzureEndpointKey) -> Self {
        key.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{hash_of, key};

    #[test]
    fn a_written_key_round_trips() {
        for (written, canonical) in [
            ("acct.blob.core.windows.net", "acct.blob.core.windows.net"),
            (
                "MyCompany.blob.core.windows.net",
                "mycompany.blob.core.windows.net",
            ),
            (
                "acct.blob.core.windows.net:443",
                "acct.blob.core.windows.net:443",
            ),
            ("proxy.internal/accta", "proxy.internal/accta"),
            ("Proxy.Internal./accta", "proxy.internal/accta"),
            ("ünï.blob.example/accta", "xn--n-nga1b.blob.example/accta"),
            (
                "[0:0:0:0:0:0:0:1]:10000/devstoreaccount1",
                "[::1]:10000/devstoreaccount1",
            ),
            (
                "127.0.0.1:10000/devstoreaccount1",
                "127.0.0.1:10000/devstoreaccount1",
            ),
            ("0x7f.1/devstoreaccount1", "127.0.0.1/devstoreaccount1"),
        ] {
            let parsed = key(written);
            assert_eq!(parsed.to_string(), canonical, "{written}");
            assert_eq!(key(canonical), parsed, "{written}");
            assert_eq!(hash_of(&key(canonical)), hash_of(&parsed), "{written}");
        }
    }

    #[test]
    fn rejected_keys() {
        let inputs = [
            // names past the account
            "acct.blob.core.windows.net/general/noarch",
            // inherits the channel URL rejections
            "acct.blob.core.windows.net@evil.example",
            "acct.blob.core.windows.net/../accta",
            r"az://acct.blob.core.windows.net/general\..\..\evil/x",
            "acct.blob.example//accta",
            "acct.blob.example/acc%zz",
            "proxy.internal/accta?sv=token",
            "proxy.internal/accta#frag",
            "acct.blob.core.windows.net:",
            "acct..blob.core.windows.net",
            // names no account
            "127.0.0.1:10000",
            "[::1]:10000",
            "localhost",
            "localhost.",
            "azurite:10000",
            "--as-user.blob.core.windows.net",
            "acct-1.blob.example",
            // the account is held to Azure's rules
            "127.0.0.1:10000/devstore;evil",
            "127.0.0.1:10000/DevStoreAccount1",
            "127.0.0.1:10000/dev-store",
            "127.0.0.1:10000/ab",
            "127.0.0.1:10000/-o",
            "127.0.0.1:10000/--as-user",
        ];

        let rejections: indexmap::IndexMap<&str, String> = inputs
            .iter()
            .map(|written| match AzureEndpointKey::parse(written) {
                Ok(_) => panic!("expected a rejection for {written}"),
                Err(err) => (*written, err.to_string()),
            })
            .collect();
        insta::assert_yaml_snapshot!(rejections);
    }

    // A host-style key cannot be made of a host without multiple 'dot' sections,
    // i.e. we require {account}.{rest}
    #[test]
    fn a_host_style_key_rejects_undottable_hosts() {
        for host in [
            "127.0.0.1:10000",
            "azurite:10000",
            "[::1]:10000",
            "localhost",
            "localhost.",
            "azurite:443",
            "azurite:80",
        ] {
            let err = AzureEndpointKey::host_style(&AzureHost::parse(host).unwrap())
                .expect_err("a host-style key must not accept an undottable host");
            assert!(matches!(err, AzureUrlError::InvalidHost(_)), "{err}");
        }
    }
}
