use indexmap::IndexMap;
use rattler_azure::{AzureEndpointOptions, AzureHost, AzureScheme};
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
/// An entry is a *grant*: it is the only way a container gets credentials, or a
/// host a non-default scheme or path-style addressing. A host with no entry is
/// fetched anonymously over https in host-style addressing, so an empty map is the
/// safe default and [`AzureOptionsMap::get`] can answer for absent hosts too.
///
/// The grant itself is keyed per container *inside* the entry (see
/// [`AzureEndpointOptions`]), because Azure assigns RBAC per container. Container
/// names need none of the normalization the host key below is about: Azure allows
/// only lowercase in one, so a container has exactly one spelling and two keys that
/// mean the same container cannot be written.
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
        self.0.get(host).cloned().unwrap_or_default()
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
    /// needed: editing an entry is [`get`](Self::get), change, insert. That copies
    /// the entry's grant table, which is not worth a second mutable path into a
    /// private map — config editing happens once per `config set`, not per request.
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

    /// The entries as the fetch middleware takes them, ready to hand to
    /// `AzureMiddleware::new` without a caller rebuilding a map by hand.
    ///
    /// Whole entries, not the narrower `AzureFetchOptions`: the middleware has to
    /// read a host's addressing before it can tell which path segment is the
    /// container, and only then can it look the container's grant up. The narrowing
    /// therefore happens per request, inside the middleware, and not here.
    pub fn endpoint_options(&self) -> impl Iterator<Item = (AzureHost, AzureEndpointOptions)> {
        self.0
            .iter()
            .map(|(host, options)| (host.clone(), options.clone()))
    }
}

/// Reject a document that spells one host two ways.
///
/// Both spellings reach serde, which keeps whichever the table iterated last —
/// silently dropping one spelling's whole entry, grants and all. TOML's own
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
        // Merge the two maps, with `other`'s entries layered over existing keys.
        // The host-scoped fields — `scheme`, `path-style` — replace wholesale, but
        // the grants merge per container: a higher-precedence file naming one
        // container must not discard a grant a lower file made on a different
        // container it never mentions. It can still revoke the container it does
        // name, because an explicit `false` is a legal grant.
        let mut merged = self.0;
        for (key, value) in &other.0 {
            let layered = match merged.get(key) {
                Some(lower) => value.layered_over(lower),
                None => value.clone(),
            };
            merged.insert(key.clone(), layered);
        }
        Ok(AzureOptionsMap(merged))
    }

    fn validate(&self) -> Result<(), super::ValidationError> {
        for (host, options) in &self.0 {
            if options.endpoint().scheme != AzureScheme::Http || is_local(host) {
                continue;
            }
            // One granted container is enough: the scheme is host-scoped, so its
            // requests all ride the same cleartext connection.
            if let Some((container, _)) = options.grants().find(|(_, auth)| auth.is_granted()) {
                return Err(super::ValidationError::Invalid(format!(
                    "`azure-options.\"{host}\".auth` grants credentials to `{container}` over \
                     cleartext http. A credential may only be sent unencrypted to a local \
                     endpoint: use an https scheme, or address the emulator by loopback address."
                )));
            }
        }
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        // Quoted, because every Azure authority contains dots and an unquoted key
        // is not the TOML path the user must pass to `config set`/`unset`. A
        // container name never needs quoting — Azure's rules leave nothing in one
        // that a bare TOML key cannot hold.
        //
        // The per-container grants are listed as their own keys so each is
        // separately unsettable. There is deliberately no `."<host>".auth` key: the
        // path exists only as a table of containers, so `config set
        // azure-options."<host>".auth true` has nowhere to land — which is the
        // point, since that is the one edit whose blast radius would be the whole
        // account.
        self.0
            .iter()
            .flat_map(|(host, options)| {
                let host = toml::Value::from(host.to_string()).to_string();
                let grants = options
                    .grants()
                    .map(|(container, _)| format!("{host}.auth.{container}"))
                    .collect::<Vec<_>>();
                std::iter::once(host).chain(grants)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use rattler_azure::{Addressing, Auth, AzureScheme, ContainerName};

    use super::*;

    fn host(authority: &str) -> AzureHost {
        AzureHost::parse(authority).expect("test host should parse")
    }

    fn container(name: &str) -> ContainerName {
        ContainerName::new(name).expect("test container name")
    }

    /// A grant can be written and revoked without the inner map being public, and
    /// a revoked host falls back to anonymous rather than lingering.
    #[test]
    fn a_grant_can_be_written_and_revoked() {
        let key = host("mycompany.blob.core.windows.net");
        let granted = AzureEndpointOptions::new(
            [(container("releases"), Auth::DefaultChain)],
            rattler_azure::AzureEndpoint::default(),
        );

        let mut map = AzureOptionsMap::default();
        assert!(map.is_empty());
        assert_eq!(map.insert(key.clone(), granted.clone()), None);
        assert_eq!(map.get(&key), granted);
        assert!(!map.is_empty());

        assert_eq!(map.remove(&key), Some(granted));
        assert!(
            !map.get(&key)
                .fetch(Some(&container("releases")))
                .auth
                .is_granted()
        );
        assert!(map.is_empty());
        assert_eq!(map.remove(&key), None);
    }

    /// The table parses in the shape documented for users, and an absent host
    /// answers with the anonymous defaults rather than requiring a presence check.
    #[test]
    fn table_parses_and_absent_hosts_default() {
        let map: AzureOptionsMap = toml::from_str(
            r#"
            ["mycompany.blob.core.windows.net".auth]
            releases = true

            ["127.0.0.1:10000"]
            scheme = "http"
            path-style = true

            ["127.0.0.1:10000".auth]
            general = true
            "#,
        )
        .unwrap();

        let real = map.get(&host("mycompany.blob.core.windows.net"));
        assert_eq!(
            real.fetch(Some(&container("releases"))).auth,
            Auth::DefaultChain
        );
        assert_eq!(real.endpoint().scheme, AzureScheme::Https);
        assert_eq!(real.endpoint().addressing, Addressing::HostStyle);

        let azurite = map.get(&host("127.0.0.1:10000"));
        assert_eq!(
            azurite.fetch(Some(&container("general"))).auth,
            Auth::DefaultChain
        );
        assert_eq!(azurite.endpoint().scheme, AzureScheme::Http);
        assert_eq!(azurite.endpoint().addressing, Addressing::PathStyle);

        // Neither a container the account never granted nor an unlisted host gets
        // anything, and the two are the same answer by construction.
        assert!(!real.fetch(Some(&container("public"))).auth.is_granted());
        let unlisted = map.get(&host("someoneelse.blob.core.windows.net"));
        assert!(
            !unlisted
                .fetch(Some(&container("releases")))
                .auth
                .is_granted()
        );
        assert_eq!(unlisted, AzureEndpointOptions::default());

        // The table feeds the fetch middleware directly, keys and all.
        assert_eq!(
            map.endpoint_options().collect::<Vec<_>>(),
            vec![
                (host("127.0.0.1:10000"), azurite),
                (host("mycompany.blob.core.windows.net"), real),
            ]
        );
    }

    /// A grant may only ride cleartext to an endpoint that is not routable off
    /// the machine or its LAN.
    #[test]
    fn cleartext_grants_are_confined_to_local_endpoints() {
        for authority in ["127.0.0.1:10000", "[::1]:10000", "azurite:10000"] {
            let map: AzureOptionsMap = toml::from_str(&format!(
                "[\"{authority}\"]\nscheme = \"http\"\n[\"{authority}\".auth]\ngeneral = true\n"
            ))
            .unwrap();
            assert!(map.validate().is_ok(), "{authority} is local");
        }

        for authority in ["mycompany.blob.core.windows.net", "internal.example.com"] {
            let map: AzureOptionsMap = toml::from_str(&format!(
                "[\"{authority}\"]\nscheme = \"http\"\n[\"{authority}\".auth]\npublic = false\nreleases = true\n"
            ))
            .unwrap();
            let err = map.validate().expect_err("{authority} is routable");
            // The message names the container at fault: the entry may hold many, and
            // only the granted ones are the problem.
            assert!(err.to_string().contains("releases"), "{err}");

            // The same host over https, and the same cleartext scheme with no
            // container granted, are both fine — it is only the pair that is
            // refused, and an explicit `false` is not a grant.
            let https: AzureOptionsMap =
                toml::from_str(&format!("[\"{authority}\".auth]\nreleases = true\n")).unwrap();
            assert!(https.validate().is_ok());
            let anonymous: AzureOptionsMap = toml::from_str(&format!(
                "[\"{authority}\"]\nscheme = \"http\"\n[\"{authority}\".auth]\nreleases = false\n"
            ))
            .unwrap();
            assert!(anonymous.validate().is_ok());
        }
    }

    /// Two spellings of one host must be refused rather than one silently
    /// winning: the loser here is an explicit `releases = false`.
    #[test]
    fn a_document_naming_one_host_twice_is_refused() {
        let document = r#"
[azure-options."acct.blob.example".auth]
releases = false

[azure-options."ACCT.blob.example.".auth]
releases = true
"#;
        let error = ensure_no_colliding_hosts(&document.parse().unwrap())
            .expect_err("a collision must be reported");
        assert!(error.contains("acct.blob.example"), "{error}");
    }

    /// A later config file replaces a host's endpoint wholesale. It must not be
    /// able to keep an earlier `path-style = true` while changing only the scheme.
    #[test]
    fn merge_replaces_the_endpoint_wholesale() {
        let base: AzureOptionsMap =
            toml::from_str("[\"host.example\"]\npath-style = true\n").unwrap();
        let over: AzureOptionsMap =
            toml::from_str("[\"host.example\"]\nscheme = \"http\"\n").unwrap();

        let merged = base.merge_config(&over).unwrap();
        let entry = merged.get(&host("host.example"));
        assert_eq!(entry.endpoint().scheme, AzureScheme::Http);
        assert_eq!(entry.endpoint().addressing, Addressing::HostStyle);
    }

    /// The grants, unlike the endpoint, merge per container: a user file naming one
    /// container must not silently revoke a grant a system file made on another —
    /// and, because an explicit `false` is legal, it can still revoke the one it
    /// does name. Without that, per-container merging would be a one-way ratchet.
    #[test]
    fn merge_layers_grants_per_container() {
        let system: AzureOptionsMap =
            toml::from_str("[\"host.example\".auth]\nreleases = true\nstaging = true\n").unwrap();
        let user: AzureOptionsMap =
            toml::from_str("[\"host.example\".auth]\nstaging = false\ninternal = true\n").unwrap();

        let entry = system
            .merge_config(&user)
            .unwrap()
            .get(&host("host.example"));
        assert!(
            entry.fetch(Some(&container("releases"))).auth.is_granted(),
            "a grant the user file never mentions must survive the merge"
        );
        assert!(
            !entry.fetch(Some(&container("staging"))).auth.is_granted(),
            "an explicit `false` must revoke a lower-precedence grant"
        );
        assert!(entry.fetch(Some(&container("internal"))).auth.is_granted());
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
                toml::from_str(&format!("[\"{written}\".auth]\nreleases = true\n")).unwrap();

            assert!(
                map.get(&host(looked_up))
                    .fetch(Some(&container("releases")))
                    .auth
                    .is_granted(),
                "the grant written as `{written}` did not apply to `{looked_up}`"
            );
            // The key is stored canonically, so `keys()` reports what a lookup
            // would need rather than what happened to be typed — quoted, as the
            // TOML path it has to be written as, with one key per grant so each is
            // separately settable — and writing the table back out produces a key
            // that parses to the same host.
            assert_eq!(
                map.keys(),
                vec![
                    format!("\"{looked_up}\""),
                    format!("\"{looked_up}\".auth.releases"),
                ],
                "{written}"
            );
            let written_back = toml::to_string(&map).unwrap();
            assert!(
                written_back.contains(&format!("[\"{looked_up}\"")),
                "{written} was written back as {written_back}"
            );
        }
    }

    /// A key that cannot be a host is a config error worth naming, not an entry
    /// that silently never matches.
    #[test]
    fn an_unparseable_key_is_rejected() {
        let err = toml::from_str::<AzureOptionsMap>(
            "[\"acct.blob.example/general\".auth]\nreleases = true\n",
        )
        .expect_err("a key carrying a path must be rejected");
        assert!(
            err.to_string().contains("acct.blob.example/general"),
            "{err}"
        );
    }

    /// The same rule one level down: a container key Azure would refuse is a config
    /// error, since it can never match a request either.
    #[test]
    fn an_unusable_container_key_is_rejected() {
        let err =
            toml::from_str::<AzureOptionsMap>("[\"acct.blob.example\".auth]\nReleases = true\n")
                .expect_err("an uppercase container name must be rejected");
        assert!(err.to_string().contains("Releases"), "{err}");
    }
}
