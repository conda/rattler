use crate::host::reject_stripped_characters;
use crate::{AccountName, AzureHost, AzureUrlError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Style {
    Host,
    Path,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AzureEndpointKey {
    style: Style,
    host: AzureHost,
    account: AccountName,
}

impl AzureEndpointKey {
    /// Parses `<host>`, whose first label is the account, or `<host>/<account>`.
    pub fn parse(key: &str) -> Result<Self, AzureUrlError> {
        reject_stripped_characters(key)?;
        let Some((authority, account)) = key.split_once('/') else {
            return Self::host_style(&AzureHost::parse(key)?);
        };
        let reason = if account.is_empty() {
            Some("nothing follows the `/`")
        } else if account.contains('/') {
            Some("it has more than one `/`")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(AzureUrlError::InvalidKey {
                key: key.to_string(),
                reason,
            });
        }
        Self::path_style(AzureHost::parse(authority)?, account)
    }

    pub fn host_style(host: &AzureHost) -> Result<Self, AzureUrlError> {
        let label = host
            .account_label()
            .ok_or_else(|| AzureUrlError::InvalidHost(host.to_string()))?;
        Ok(Self {
            style: Style::Host,
            host: host.clone(),
            account: AccountName::new(label)?,
        })
    }

    /// `segment` is the still-percent-encoded account path segment.
    pub fn path_style(host: AzureHost, segment: &str) -> Result<Self, AzureUrlError> {
        Ok(Self {
            style: Style::Path,
            host,
            account: AccountName::from_segment(segment)?,
        })
    }

    pub fn host(&self) -> &AzureHost {
        &self.host
    }

    pub fn account(&self) -> &AccountName {
        &self.account
    }

    /// How many leading path segments of a channel URL the key spells.
    pub(crate) fn path_segments(&self) -> usize {
        match self.style {
            Style::Host => 0,
            Style::Path => 1,
        }
    }
}

impl std::fmt::Display for AzureEndpointKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.style {
            Style::Host => write!(f, "{}", self.host),
            Style::Path => write!(f, "{}/{}", self.host, self.account),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{hash_of, key};

    #[test]
    fn a_written_key_round_trips() {
        let inputs = [
            "acct.blob.core.windows.net",
            "MyCompany.blob.core.windows.net",
            "acct.blob.core.windows.net:443",
            "proxy.internal/accta",
            "Proxy.Internal./accta",
            "127.0.0.1:10000/acc%74",
            "ünï.blob.example/accta",
            "[0:0:0:0:0:0:0:1]:10000/devstoreaccount1",
            "127.0.0.1:10000/devstoreaccount1",
            "0x7f.1/devstoreaccount1",
        ];

        let canonical: indexmap::IndexMap<&str, String> = inputs
            .iter()
            .map(|written| {
                let parsed = key(written);
                let canonical = parsed.to_string();
                assert_eq!(key(&canonical), parsed, "{written}");
                assert_eq!(hash_of(&key(&canonical)), hash_of(&parsed), "{written}");
                (*written, canonical)
            })
            .collect();
        insta::assert_yaml_snapshot!(canonical);
    }

    #[test]
    fn rejected_keys() {
        let inputs = [
            "acct.blob.core.windows.net/",
            "127.0.0.1:10000/devstoreaccount1/",
            "proxy.internal/accta/",
            "acct.blob.core.windows.net/general/noarch",
            "proxy.internal/accta/general",
            "proxy.internal/accta?sv=token",
            "proxy.internal/accta#frag",
            "acct.blob.core.windows.net@evil.example",
            "acct.blob.core.windows.net/../accta",
            r"acct.blob.core.windows.net/general\..\evil",
            "acct.blob.example//accta",
            "acct.blob.example/acc%zz",
            "acct.blob.core.windows.net:",
            "acct..blob.core.windows.net",
            "127.0.0.1:10000",
            "[::1]:10000",
            "localhost",
            "localhost.",
            "azurite:10000",
            "--as-user.blob.core.windows.net",
            "acct-1.blob.example",
            "127.0.0.1:10000/devstore;evil",
            "127.0.0.1:10000/DevStoreAccount1",
            "127.0.0.1:10000/dev-store",
            "127.0.0.1:10000/ab",
            "127.0.0.1:10000/--as-user",
            "proxy.internal/accta\\",
            "proxy.internal/accta/ ",
            "proxy.internal/accta/\n",
            "acct.blob.core.windows.net/ ",
            "acct.blob.core.windows.net\\",
            "proxy.internal/acc\nta",
            "proxy.internal\\accta",
            "acc\tt.blob.core.windows.net",
            "/accta",
        ];

        let rejections: indexmap::IndexMap<&str, String> = inputs
            .iter()
            .map(|written| match AzureEndpointKey::parse(written) {
                Ok(key) => panic!("expected a rejection for {written}, parsed as {key}"),
                Err(err) => (*written, err.to_string()),
            })
            .collect();
        insta::assert_yaml_snapshot!(rejections);
    }
}
