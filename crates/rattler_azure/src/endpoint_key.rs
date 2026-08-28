use crate::{AccountName, AzureChannelUrl, AzureHost, AzureUrlError};

/// Only [`new`](Self::new) builds one, so a host that carries no usable account
/// label — an IP literal, a single-label name, a first label Azure would refuse —
/// has no host-style spelling at all.
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

/// Only [`new`](Self::new) builds one, so the segment has passed Azure's naming
/// rules wherever it came from — a written config key or a channel URL's path.
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

/// The key of an `azure-options` entry: a channel URL prefix that runs up to, but
/// not including, the container.
///
/// Its shape is what says where the storage account is, so nothing else has to.
/// `acct.blob.core.windows.net` reads the account off the host;
/// `proxy.internal/accta` reads it from the first path segment, which is the only
/// spelling that works for an IP literal or a single-label host, and the only one
/// that tells two accounts behind one proxy apart. Under both, the container is
/// the segment right after the key.
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
            ("acct.blob.core.windows.net.", "acct.blob.core.windows.net"),
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
        ] {
            let parsed = key(written);
            assert_eq!(parsed.to_string(), canonical, "{written}");
            assert_eq!(key(canonical), parsed, "{written}");
            assert_eq!(hash_of(&key(canonical)), hash_of(&parsed), "{written}");
        }
    }

    #[test]
    fn a_key_past_the_account_is_rejected() {
        for written in [
            "proxy.internal/accta/general",
            "acct.blob.core.windows.net/general/noarch",
        ] {
            assert!(
                matches!(
                    AzureEndpointKey::parse(written),
                    Err(AzureUrlError::InvalidKey(_))
                ),
                "{written} names past the account"
            );
        }
    }

    #[test]
    fn a_key_inherits_the_channel_url_rejections() {
        for written in [
            "acct.blob.core.windows.net@evil.example",
            "acct.blob.core.windows.net/../accta",
            "acct.blob.example//accta",
            "acct.blob.example/acc%zz",
            "proxy.internal/accta?sv=token",
            "proxy.internal/accta#frag",
            "acct.blob.core.windows.net:",
            "acct..blob.core.windows.net",
        ] {
            assert!(
                AzureEndpointKey::parse(written).is_err(),
                "expected a rejection for {written}"
            );
        }
    }

    /// A key that names no account could not say which account a grant is for, so
    /// there is no such key to write.
    #[test]
    fn a_key_must_name_an_account() {
        for written in [
            "127.0.0.1:10000",
            "[::1]:10000",
            "localhost",
            "localhost.",
            "azurite:10000",
            "--as-user.blob.core.windows.net",
            "acct-1.blob.example",
        ] {
            assert!(
                AzureEndpointKey::parse(written).is_err(),
                "expected a rejection for {written}"
            );
        }
    }

    #[test]
    fn a_backslash_spelled_dot_segment_is_rejected() {
        for input in [
            r"az://acct.blob.core.windows.net/general\..\..\evil/x",
            r"az://127.0.0.1:10000/devstoreaccount1/general\..\..\otheracct\othercontainer",
            r"az://acct.blob.core.windows.net\general\.\noarch",
        ] {
            assert!(
                matches!(
                    AzureChannelUrl::parse(input),
                    Err(AzureUrlError::DotSegmentInPath(_))
                ),
                "expected a rejection for {input}"
            );
        }

        for written in [
            r"proxy.internal/x\..\..\accta",
            "proxy.internal/x/../../accta",
        ] {
            assert!(
                AzureEndpointKey::parse(written).is_err(),
                "expected a rejection for {written}"
            );
        }
    }

    #[test]
    fn an_account_a_key_names_is_held_to_azures_rules() {
        for written in [
            "127.0.0.1:10000/devstore;evil",
            "127.0.0.1:10000/DevStoreAccount1",
            "127.0.0.1:10000/dev-store",
            "127.0.0.1:10000/ab",
            "127.0.0.1:10000/-o",
            "127.0.0.1:10000/--as-user",
        ] {
            assert!(
                matches!(
                    AzureEndpointKey::parse(written),
                    Err(AzureUrlError::InvalidAccountName(_))
                ),
                "expected a rejection for {written}"
            );
        }
    }

    #[test]
    fn a_host_style_key_rejects_undottable_hosts() {
        for host in [
            "127.0.0.1:10000",
            "azurite:10000",
            "localhost",
            "[::1]:10000",
            // A trailing dot is the DNS root label, not a second label: this host
            // must not sneak past the dotted-domain gate.
            "localhost.",
            "LocalHost",
            "azurite:443",
            "azurite:80",
        ] {
            let err = AzureEndpointKey::host_style(&AzureHost::parse(host).unwrap())
                .expect_err("a host-style key must not accept an undottable host");
            assert!(matches!(err, AzureUrlError::InvalidHost(_)), "{err}");
            assert!(err.to_string().contains("/<account>"), "{err}");
        }
    }
}
