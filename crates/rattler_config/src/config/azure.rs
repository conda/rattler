use indexmap::IndexMap;
use rattler_azure::{Auth, AzureEndpointOptions, AzureFetchOptions, AzureHost, AzureScheme};
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Whether a credential may cross this host's network unencrypted.
///
/// A single-label name (`localhost`, a `docker compose` service) has no public DNS
/// resolution, so it counts as local; anything with a dot does not.
fn is_local(host: &AzureHost) -> bool {
    match host.host() {
        url::Host::Domain(domain) => !domain.contains('.'),
        url::Host::Ipv4(ip) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
        }
        url::Host::Ipv6(ip) => ip.is_loopback() || ip.is_unspecified(),
    }
}

/// Per-host options for Azure Blob channels, keyed by endpoint authority
/// (including a port where one is used, e.g. `127.0.0.1:10000`).
///
/// An entry is a *grant*: it is the only way a host gets credentials, a
/// non-default scheme, or path-style addressing. A host with no entry is fetched
/// anonymously over https in host-style addressing, so an empty map is the safe
/// default and [`AzureOptionsMap::get`] can answer for absent hosts too.
///
/// # Why the key is an [`AzureHost`] and not a `String`
///
/// A silently-missed grant is the worst failure this table has: Azure answers an
/// unauthorized request for a private container with a 404, so the user is told
/// "not found" rather than "not authorized". Keyed by raw TOML text, every host
/// normalization is such a miss — `MyCompany.blob…` , `host:443`, `ünï.blob…`,
/// `[0:0:0:0:0:0:0:1]:10000` and `0x7f.1` are all spellings a lookup would arrive
/// with in a different form. Keying by [`AzureHost`] deletes the class: the key is
/// deserialized through the same parser that produces the lookup value, so the two
/// cannot disagree. The inner map is private for the same reason — a key that did
/// not go through that parser must be unrepresentable, not merely discouraged.
///
/// # Scope
///
/// Entries are **user-scoped by contract**. A tool must never read this table
/// from a project- or workspace-level manifest: doing so would let a checked-out
/// repository name a host and have the user's ambient Azure credentials sent to
/// it. Keep it to user- and system-level config files.
#[derive(Default, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AzureOptionsMap(IndexMap<AzureHost, AzureEndpointOptions>);

impl AzureOptionsMap {
    /// The options for `host`, or the defaults (anonymous, https, host-style)
    /// when it has no entry.
    ///
    /// Callers should prefer this over indexing the map: "no entry" and "a
    /// defaulted entry" are defined to behave identically, so branching on
    /// presence only invites the two paths to drift apart.
    pub fn get(&self, host: &AzureHost) -> AzureEndpointOptions {
        self.0.get(host).copied().unwrap_or_default()
    }

    /// The configured hosts, in the order the document's table iterated them
    /// (`toml::Table` is a `BTreeMap`, so that is byte order, not write order).
    pub fn hosts(&self) -> impl Iterator<Item = &AzureHost> {
        self.0.keys()
    }

    /// Whether no host is configured, which is also "every `az://` host is
    /// anonymous". Serializers skip the table on this.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Grant `host` these options, returning what it was granted before.
    ///
    /// Taking an [`AzureHost`] rather than a string is what lets the inner map stay
    /// private while still being writable: a caller editing config (`pixi config
    /// set azure-options."…"`) has to have parsed its key, so it cannot install an
    /// entry a lookup would fail to find. There is no `get_mut`, and none is
    /// needed — [`AzureEndpointOptions`] is `Copy`, so editing one field is
    /// [`get`](Self::get), change, insert.
    pub fn insert(
        &mut self,
        host: AzureHost,
        options: AzureEndpointOptions,
    ) -> Option<AzureEndpointOptions> {
        self.0.insert(host, options)
    }

    /// Revoke `host`'s grant, returning it if there was one.
    ///
    /// Shift-removes, so the remaining entries keep their relative order and a
    /// serialized table does not reshuffle on an unrelated edit.
    pub fn remove(&mut self, host: &AzureHost) -> Option<AzureEndpointOptions> {
        self.0.shift_remove(host)
    }

    /// The grants as the fetch path takes them, ready to hand to
    /// `AzureMiddleware::new` without a caller rebuilding a map by hand.
    pub fn fetch_options(&self) -> impl Iterator<Item = (AzureHost, AzureFetchOptions)> {
        self.0
            .iter()
            .map(|(host, options)| (host.clone(), options.fetch()))
    }
}

/// Reject a document that spells one host two ways.
///
/// Both spellings reach serde, which keeps whichever the table iterated last —
/// silently overriding an `auth = false` or dropping a grant. TOML's own
/// duplicate-key check runs on the raw text, so it cannot see the collision; this
/// has to run while both spellings are still visible.
pub(crate) fn ensure_no_colliding_hosts(document: &toml::Table) -> Result<(), String> {
    let Some(table) = document
        .get("azure-options")
        .and_then(toml::Value::as_table)
    else {
        return Ok(());
    };

    let mut seen: IndexMap<AzureHost, &String> = IndexMap::new();
    for written in table.keys() {
        // An unparseable key is serde's error to report, not ours.
        let Ok(host) = AzureHost::parse(written) else {
            continue;
        };
        if let Some(first) = seen.insert(host.clone(), written) {
            return Err(format!(
                "`azure-options` names one host twice: \"{first}\" and \"{written}\" are both \
                 `{host}`"
            ));
        }
    }
    Ok(())
}

impl Config for AzureOptionsMap {
    fn is_default(&self) -> bool {
        self.0.is_empty()
    }

    fn merge_config(self, other: &Self) -> Result<Self, super::MergeError> {
        // Merge the two maps, with `other`'s entries overwriting existing keys.
        // A host is granted or not as a whole, so mentioning a host in a
        // higher-precedence file replaces the lower file's entry outright rather
        // than merging field-wise the way `repodata-config` does.
        let mut merged = self.0;
        for (key, value) in &other.0 {
            merged.insert(key.clone(), *value);
        }
        Ok(AzureOptionsMap(merged))
    }

    fn validate(&self) -> Result<(), super::ValidationError> {
        for (host, options) in &self.0 {
            let fetch = options.fetch();
            if fetch.auth == Auth::DefaultChain
                && fetch.scheme == AzureScheme::Http
                && !is_local(host)
            {
                return Err(super::ValidationError::Invalid(format!(
                    "`azure-options.\"{host}\"` grants credentials over cleartext http. A \
                     credential may only be sent unencrypted to a local endpoint: use an \
                     https scheme, or address the emulator by loopback address."
                )));
            }
        }
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        // Quoted, because every Azure authority contains dots and an unquoted key
        // is not the TOML path the user must pass to `config set`/`unset`.
        self.0
            .keys()
            .map(|host| toml::Value::from(host.to_string()).to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use rattler_azure::{Addressing, Auth, AzureScheme};

    use super::*;

    fn host(authority: &str) -> AzureHost {
        AzureHost::parse(authority).expect("test host should parse")
    }

    /// A grant can be written and revoked without the inner map being public, and
    /// a revoked host falls back to anonymous rather than lingering.
    #[test]
    fn a_grant_can_be_written_and_revoked() {
        let key = host("mycompany.blob.core.windows.net");
        let granted =
            AzureEndpointOptions::new(Auth::DefaultChain, rattler_azure::AzureEndpoint::default());

        let mut map = AzureOptionsMap::default();
        assert!(map.is_empty());
        assert_eq!(map.insert(key.clone(), granted), None);
        assert_eq!(map.get(&key), granted);
        assert!(!map.is_empty());

        assert_eq!(map.remove(&key), Some(granted));
        assert!(!map.get(&key).fetch().auth.is_granted());
        assert!(map.is_empty());
        assert_eq!(map.remove(&key), None);
    }

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

        let real = map.get(&host("mycompany.blob.core.windows.net"));
        assert_eq!(real.fetch().auth, Auth::DefaultChain);
        assert_eq!(real.endpoint().scheme, AzureScheme::Https);
        assert_eq!(real.endpoint().addressing, Addressing::HostStyle);

        let azurite = map.get(&host("127.0.0.1:10000"));
        assert_eq!(azurite.fetch().auth, Auth::DefaultChain);
        assert_eq!(azurite.endpoint().scheme, AzureScheme::Http);
        assert_eq!(azurite.endpoint().addressing, Addressing::PathStyle);

        // An unlisted host gets no grant.
        let unlisted = map.get(&host("someoneelse.blob.core.windows.net"));
        assert!(!unlisted.fetch().auth.is_granted());
        assert_eq!(unlisted, AzureEndpointOptions::default());

        // The table feeds the fetch middleware directly, keys and all.
        assert_eq!(
            map.fetch_options().collect::<Vec<_>>(),
            vec![
                (host("127.0.0.1:10000"), azurite.fetch()),
                (host("mycompany.blob.core.windows.net"), real.fetch()),
            ]
        );
    }

    /// A grant may only ride cleartext to an endpoint that is not routable off
    /// the machine or its LAN.
    #[test]
    fn cleartext_grants_are_confined_to_local_endpoints() {
        for authority in ["127.0.0.1:10000", "[::1]:10000", "azurite:10000"] {
            let map: AzureOptionsMap = toml::from_str(&format!(
                "[\"{authority}\"]\nauth = true\nscheme = \"http\"\n"
            ))
            .unwrap();
            assert!(map.validate().is_ok(), "{authority} is local");
        }

        for authority in ["mycompany.blob.core.windows.net", "internal.example.com"] {
            let map: AzureOptionsMap = toml::from_str(&format!(
                "[\"{authority}\"]\nauth = true\nscheme = \"http\"\n"
            ))
            .unwrap();
            assert!(map.validate().is_err(), "{authority} is routable");

            // The same host over https, and the same cleartext scheme without a
            // grant, are both fine — it is only the pair that is refused.
            let https: AzureOptionsMap =
                toml::from_str(&format!("[\"{authority}\"]\nauth = true\n")).unwrap();
            assert!(https.validate().is_ok());
            let anonymous: AzureOptionsMap =
                toml::from_str(&format!("[\"{authority}\"]\nscheme = \"http\"\n")).unwrap();
            assert!(anonymous.validate().is_ok());
        }
    }

    /// Two spellings of one host must be refused rather than one silently
    /// winning: the loser here is an explicit `auth = false`.
    #[test]
    fn a_document_naming_one_host_twice_is_refused() {
        let document = r#"
[azure-options."acct.blob.example"]
auth = false

[azure-options."ACCT.blob.example."]
auth = true
"#;
        let error = ensure_no_colliding_hosts(&document.parse().unwrap())
            .expect_err("a collision must be reported");
        assert!(error.contains("acct.blob.example"), "{error}");
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
        let entry = merged.get(&host("host.example"));
        assert_eq!(entry.endpoint().scheme, AzureScheme::Http);
        assert!(
            !entry.fetch().auth.is_granted(),
            "overwriting an entry must not inherit the previous grant"
        );
        assert_eq!(entry.endpoint().addressing, Addressing::HostStyle);
    }

    /// The defect this key type exists to kill: every one of these keys is a
    /// spelling a lookup arrives with in normalized form, and with a `String` key
    /// each was a silent miss — an anonymous fetch, a 404, and a user told "not
    /// found" instead of "not authorized".
    #[test]
    fn keys_are_normalized_the_same_way_lookups_are() {
        for (written, looked_up) in [
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
            let map: AzureOptionsMap =
                toml::from_str(&format!("[\"{written}\"]\nauth = true\n")).unwrap();

            assert!(
                map.get(&host(looked_up)).fetch().auth.is_granted(),
                "the grant written as `{written}` did not apply to `{looked_up}`"
            );
            // The key is stored canonically, so `keys()` reports what a lookup
            // would need rather than what happened to be typed — quoted, as the
            // TOML path it has to be written as — and writing the table back out
            // produces a key that parses to the same host.
            assert_eq!(map.keys(), vec![format!("\"{looked_up}\"")], "{written}");
            let written_back = toml::to_string(&map).unwrap();
            assert!(
                written_back.contains(&format!("[\"{looked_up}\"]")),
                "{written} was written back as {written_back}"
            );
        }
    }

    /// A key that cannot be a host is a config error worth naming, not an entry
    /// that silently never matches.
    #[test]
    fn an_unparseable_key_is_rejected() {
        let err =
            toml::from_str::<AzureOptionsMap>("[\"acct.blob.example/general\"]\nauth = true\n")
                .expect_err("a key carrying a path must be rejected");
        assert!(
            err.to_string().contains("acct.blob.example/general"),
            "{err}"
        );
    }
}
