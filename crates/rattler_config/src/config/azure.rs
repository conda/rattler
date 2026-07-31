use indexmap::IndexMap;
use rattler_azure::AzureEndpointOptions;
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Per-host options for Azure Blob channels, keyed by host (including a port
/// where one is used, e.g. `127.0.0.1:10000`).
///
/// An entry is a *grant*: it is the only way a host gets credentials, a
/// non-default scheme, or path-style addressing. A host with no entry is fetched
/// anonymously over https in host-style addressing, so an empty map is the safe
/// default and [`AzureOptionsMap::get`] can answer for absent hosts too.
///
/// # Scope
///
/// Entries are **user-scoped by contract**. A tool must never read this table
/// from a project- or workspace-level manifest: doing so would let a checked-out
/// repository name a host and have the user's ambient Azure credentials sent to
/// it. Keep it to user- and system-level config files.
#[derive(Default, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AzureOptionsMap(pub IndexMap<String, AzureEndpointOptions>);

impl AzureOptionsMap {
    /// Returns `true` if no Azure hosts are configured.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The options for `host`, or the defaults (anonymous, https, host-style)
    /// when it has no entry.
    ///
    /// Callers should prefer this over indexing the map: "no entry" and "a
    /// defaulted entry" are defined to behave identically, so branching on
    /// presence only invites the two paths to drift apart.
    pub fn get(&self, host: &str) -> AzureEndpointOptions {
        self.0.get(host).copied().unwrap_or_default()
    }
}

impl Config for AzureOptionsMap {
    fn is_default(&self) -> bool {
        self.0.is_empty()
    }

    fn merge_config(self, other: &Self) -> Result<Self, super::MergeError> {
        // Merge the two maps, with `other`'s entries overwriting existing keys.
        // A host is granted or not as a whole; entries are not merged field-wise,
        // so a higher-precedence file cannot partially relax a grant.
        let mut merged = self.0.clone();
        for (key, value) in &other.0 {
            merged.insert(key.clone(), *value);
        }
        Ok(AzureOptionsMap(merged))
    }

    fn validate(&self) -> Result<(), super::ValidationError> {
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        self.0.keys().map(ToString::to_string).collect()
    }
}

#[cfg(test)]
mod tests {
    use rattler_azure::{Addressing, Auth, Scheme};

    use super::*;

    /// The table parses in the shape documented for users, and an absent host
    /// answers with the anonymous defaults rather than requiring a presence check.
    #[test]
    fn table_parses_and_absent_hosts_default() {
        let map: AzureOptionsMap = toml::from_str(
            r#"
            ["mycompany.blob.core.windows.net"]
            auth = true

            ["127.0.0.1:10000"]
            auth = true
            scheme = "http"
            path-style = true
            "#,
        )
        .unwrap();

        let real = map.get("mycompany.blob.core.windows.net");
        assert_eq!(real.auth, Auth::DefaultChain);
        assert_eq!(real.scheme, Scheme::Https);
        assert_eq!(real.path_style, Addressing::HostStyle);

        let azurite = map.get("127.0.0.1:10000");
        assert_eq!(azurite.auth, Auth::DefaultChain);
        assert_eq!(azurite.scheme, Scheme::Http);
        assert_eq!(azurite.path_style, Addressing::PathStyle);

        // An unlisted host gets no grant.
        let unlisted = map.get("someoneelse.blob.core.windows.net");
        assert!(!unlisted.auth.is_granted());
        assert_eq!(unlisted, AzureEndpointOptions::default());
    }

    /// A later config file overwrites a host wholesale. It must not be able to
    /// keep an earlier `auth = true` while changing only the scheme.
    #[test]
    fn merge_replaces_entries_wholesale() {
        let base: AzureOptionsMap =
            toml::from_str("[\"host.example\"]\nauth = true\npath-style = true\n").unwrap();
        let over: AzureOptionsMap =
            toml::from_str("[\"host.example\"]\nscheme = \"http\"\n").unwrap();

        let merged = base.merge_config(&over).unwrap();
        let entry = merged.get("host.example");
        assert_eq!(entry.scheme, Scheme::Http);
        assert!(
            !entry.auth.is_granted(),
            "overwriting an entry must not inherit the previous grant"
        );
        assert_eq!(entry.path_style, Addressing::HostStyle);
    }
}
