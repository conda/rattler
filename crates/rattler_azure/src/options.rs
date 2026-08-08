//! Per-host endpoint options for Azure Blob channels.
//!
//! An entry in the `azure-options` config table is the *only* thing that grants
//! a host or one of its containers anything: without one, a channel on that host
//! is fetched anonymously over https in host-style addressing. There is
//! deliberately no hardcoded list of "official" Azure suffixes — since a grant must
//! be written out for the container it applies to, suffix classification carries no
//! security weight, and the absence of the list is what lets custom endpoints and
//! the Azurite emulator work at all.
//!
//! # Three types, one table
//!
//! [`AzureEndpointOptions`] is the file format. The write path never sees it: it
//! takes [`AzureEndpoint`], so a grant cannot reach a caller that supplies its own
//! credential — a field a consumer would have to ignore reads as a guarantee, and
//! the write path ignoring a grant looked exactly like a credential gate that was
//! never there.
//!
//! The fetch path holds the entry but never acts on it directly either: per request
//! it narrows to [`AzureFetchOptions`], which carries the grant for the one
//! container the request addresses and no way to address another. It cannot narrow
//! any earlier, because finding that container needs the entry's [`Addressing`]
//! first.
//!
//! # Why enums for what the config spells as bools
//!
//! The TOML surface stays `<container> = true` / `path-style = true`, because that
//! is the ergonomic spelling and it keeps the table skimmable. Internally each is an
//! enum, so no call site can mix up two unrelated booleans, and the meaning of a
//! value is legible without chasing the field name. The bridge is a
//! `serde(from = "bool", into = "bool")` pair with a `From` impl each way, applied
//! through `cfg_attr` so the serde attributes stay behind the `serde` feature
//! along with the derives they configure.
//!
//! The types themselves are always available — [`Addressing`] decides how
//! [`account_and_container`](crate::account_and_container) reads a URL and
//! [`AzureScheme`] is what [`AzureChannelUrl::wire`](crate::AzureChannelUrl::wire)
//! is spelled in, neither of which involves serde. Only the derives are behind
//! the `serde` feature, so a consumer that just wants the URL types does not
//! pull serde in.

use crate::ContainerName;

/// Whether credentials may attach to requests for a container.
///
/// Defaults to [`Auth::Anonymous`]: a container gets no credentials until a config
/// entry names it. Serializes as the bool a container is spelled with in an
/// `azure-options` `auth` table.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(from = "bool", into = "bool")
)]
pub enum Auth {
    /// Send requests unsigned. No credential is resolved, so no ambient
    /// credential can be exfiltrated to this host, and nothing blocks on the
    /// managed-identity / IMDS probe.
    #[default]
    Anonymous,

    /// Run the standard Azure credential chain for this container and sign with what
    /// it returns. Because this is an explicit grant, a broken or unusable
    /// credential is a hard error — never a silent downgrade to anonymous.
    DefaultChain,
}

impl From<bool> for Auth {
    fn from(value: bool) -> Self {
        if value {
            Auth::DefaultChain
        } else {
            Auth::Anonymous
        }
    }
}

impl From<Auth> for bool {
    fn from(value: Auth) -> Self {
        matches!(value, Auth::DefaultChain)
    }
}

impl Auth {
    /// Whether this grant permits a credential to be sent.
    pub fn is_granted(self) -> bool {
        matches!(self, Auth::DefaultChain)
    }
}

/// The wire scheme an `az://` channel URL is rewritten to when a request is sent.
///
/// Named for the crate rather than spelled bare `Scheme`, because this crate also
/// depends on `opendal`, whose own `Scheme` names a storage service — two very
/// different things one import away from each other.
///
/// Defaults to [`AzureScheme::Https`]. `Http` exists for local emulators such as
/// Azurite; choosing it is an explicit, per-host decision in config, so a plain
/// `az://` URL can never be silently downgraded to cleartext.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(rename_all = "lowercase")
)]
pub enum AzureScheme {
    /// Send requests over TLS.
    #[default]
    Https,

    /// Send requests in cleartext. For local emulators only.
    Http,
}

impl AzureScheme {
    /// The scheme as it appears in a URL, without the `://`.
    pub fn as_str(self) -> &'static str {
        match self {
            AzureScheme::Https => "https",
            AzureScheme::Http => "http",
        }
    }
}

impl std::fmt::Display for AzureScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where the storage account name is found in a blob URL.
///
/// Defaults to [`Addressing::HostStyle`], which is how real Azure addresses
/// accounts. Serializes as the bool `path-style` in `azure-options`; `s3-options`
/// spells its equivalent `force-path-style`, also a bool.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(from = "bool", into = "bool")
)]
pub enum Addressing {
    /// The account is the first label of the host, as in
    /// `<account>.blob.core.windows.net/<container>`. Requires a domain with at
    /// least two labels, so IP literals and single-label hosts cannot be
    /// addressed this way.
    #[default]
    HostStyle,

    /// The account is the first path segment, as in
    /// `<host>/<account>/<container>`. This is what Azurite and other emulators
    /// use, and it is the only form that works for an IP or single-label host.
    PathStyle,
}

impl Addressing {
    /// Which path segment holds the container name under this addressing.
    ///
    /// One number, read by both derivations that need it —
    /// [`account_and_container`](crate::account_and_container) and
    /// [`container`](crate::container). Two derivations that disagreed about which
    /// segment is the container would look a grant up for one container and apply
    /// it to another.
    pub(crate) fn container_segment(self) -> usize {
        match self {
            // `<account>.host/<container>/…`
            Addressing::HostStyle => 0,
            // `host/<account>/<container>/…`
            Addressing::PathStyle => 1,
        }
    }
}

impl From<bool> for Addressing {
    fn from(value: bool) -> Self {
        if value {
            Addressing::PathStyle
        } else {
            Addressing::HostStyle
        }
    }
}

impl From<Addressing> for bool {
    fn from(value: Addressing) -> Self {
        matches!(value, Addressing::PathStyle)
    }
}

/// How to address one Azure Blob host. Carries no grant.
///
/// This is what the write path takes. Splitting it out is what stops
/// [`Auth`] from reaching a consumer that cannot act on it: `azblob_config` and
/// the SAS mint are handed a material credential by their caller, so there is no
/// ambient chain for a grant to gate, and a grant they could read would be a
/// promise nothing keeps.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AzureEndpoint {
    /// The scheme `az://` is rewritten to for this host.
    pub scheme: AzureScheme,

    /// Where the account name is found in the URL for this host.
    pub addressing: Addressing,
}

/// What the fetch middleware needs to send one request.
///
/// [`Addressing`] is absent rather than ignored: the fetch path forwards a path
/// and never derives an account name from it. It does read the addressing to find
/// the container a grant is looked up by, but that happens one step earlier, on
/// [`AzureEndpointOptions`] — by the time this exists the grant is already
/// resolved, so there is nothing left here to address.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AzureFetchOptions {
    /// Whether credentials may be sent for the container this was resolved for.
    pub auth: Auth,

    /// The scheme `az://` is rewritten to for this host.
    pub scheme: AzureScheme,
}

/// One `azure-options` entry, as the config file spells it.
///
/// This is the serde surface and nothing else: the TOML keys live here, and each
/// consumer takes the narrower view it can actually act on, via [`Self::endpoint`]
/// or [`Self::fetch`]. The fields are private so that view is the only way in.
///
/// The default value is the no-entry behaviour: no grants, https, host-style. A
/// host with no config entry behaves exactly as if it had a defaulted entry, so
/// callers can look up an absent host and fall back to `default()` rather than
/// branching on presence.
///
/// # Why the grant is per container and the endpoint is per host
///
/// The two settings have different scopes, and it is not a matter of taste. Azure
/// assigns RBAC per *container*, so one storage account routinely holds a mix of
/// private and anonymous-read containers; a grant that could only be written per
/// host cannot express that account at all — signing the anonymous-read container
/// 403s for any identity holding no role on it, and not signing breaks the private
/// ones. `scheme` and `addressing` describe the *endpoint*: two containers on one
/// account disagreeing about where the account name lives is not a configuration,
/// it is a contradiction.
///
/// So there is deliberately no host-level `auth` field. Not "defaults to false" —
/// absent from the type, so the one setting whose blast radius would be every
/// container on the account, including containers created after it was written, is
/// unrepresentable rather than merely discouraged. The worst typo here grants one
/// container.
///
/// ```toml
/// [azure-options."mycompany.blob.core.windows.net"]
/// scheme = "https"
/// path-style = false
///
/// [azure-options."mycompany.blob.core.windows.net".auth]
/// releases = true
/// staging = true
/// # a container not listed here is fetched anonymously
/// ```
#[derive(Default, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(rename_all = "kebab-case", default)
)]
pub struct AzureEndpointOptions {
    scheme: AzureScheme,

    /// The field is named for what it holds, but the config key stays
    /// `path-style`: that is the spelling users have written, and the bool bridge
    /// is what the key means.
    #[cfg_attr(feature = "serde", serde(rename = "path-style", alias = "path_style"))]
    addressing: Addressing,

    /// Which containers on this host may be sent credentials.
    ///
    /// The value is an [`Auth`] rather than a `bool` because that is what
    /// [`AzureFetchOptions`] already speaks, so a grant flows from here to the
    /// signer without a `if granted { … } else { … }` at every boundary — one less
    /// place to invert a condition on a credential decision. The TOML surface is
    /// unaffected: the bool bridge keeps it `releases = true`.
    ///
    /// An explicit `false` is legal and redundant with omission, which is what
    /// makes a higher-precedence config file able to *revoke* a grant a lower one
    /// made (see [`Self::layered_over`]).
    ///
    /// Last, because the TOML serializer must emit an entry's scalars before its
    /// tables.
    #[cfg_attr(
        feature = "serde",
        serde(skip_serializing_if = "indexmap::IndexMap::is_empty")
    )]
    auth: indexmap::IndexMap<ContainerName, Auth>,
}

impl AzureEndpointOptions {
    /// Build an entry from its per-container grants and the endpoint they apply to.
    pub fn new(
        auth: impl IntoIterator<Item = (ContainerName, Auth)>,
        endpoint: AzureEndpoint,
    ) -> Self {
        Self {
            scheme: endpoint.scheme,
            addressing: endpoint.addressing,
            auth: auth.into_iter().collect(),
        }
    }

    /// How to address this host, for the write path.
    pub fn endpoint(&self) -> AzureEndpoint {
        AzureEndpoint {
            scheme: self.scheme,
            addressing: self.addressing,
        }
    }

    /// The grant and wire scheme for one container, for the fetch path.
    ///
    /// `container` is an `Option` because a URL need not name one: the host root,
    /// or a path too short for the addressing, has nothing to attribute a grant to.
    /// Answering that case here rather than at the call site is what keeps "no
    /// container" from being spelled two ways — it can only mean anonymous, since
    /// there is no entry it could match.
    pub fn fetch(&self, container: Option<&ContainerName>) -> AzureFetchOptions {
        AzureFetchOptions {
            auth: container
                .and_then(|container| self.auth.get(container))
                .copied()
                .unwrap_or_default(),
            scheme: self.scheme,
        }
    }

    /// Every container this entry mentions, and what it grants it.
    ///
    /// Includes the explicit `false`s: a caller validating or listing the table has
    /// to see what the file says, not what it effectively means.
    pub fn grants(&self) -> impl Iterator<Item = (&ContainerName, Auth)> {
        self.auth.iter().map(|(container, auth)| (container, *auth))
    }

    /// This entry layered over the one a lower-precedence config file wrote.
    ///
    /// `scheme` and `addressing` describe the endpoint as a whole, so this entry
    /// replaces them outright. The grants merge per container: a file naming one
    /// container must not silently drop a grant on a container it never mentions.
    /// The merge is not a one-way ratchet, because an explicit `false` is a legal
    /// grant — that is how a user file revokes what a system file granted.
    pub fn layered_over(&self, lower: &Self) -> Self {
        let mut auth = lower.auth.clone();
        auth.extend(self.auth.iter().map(|(c, auth)| (c.clone(), *auth)));
        Self {
            scheme: self.scheme,
            addressing: self.addressing,
            auth,
        }
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    fn container(name: &str) -> ContainerName {
        ContainerName::new(name).expect("test container name")
    }

    /// The whole point of the bool bridge: the TOML stays boolean while the code
    /// sees enums, and an absent field takes the safe default.
    #[test]
    fn toml_bools_map_to_enums() {
        let opts: AzureEndpointOptions = toml::from_str(
            r#"
            scheme = "http"
            path-style = true

            [auth]
            releases = true
            "#,
        )
        .unwrap();
        assert_eq!(
            opts,
            AzureEndpointOptions::new(
                [(container("releases"), Auth::DefaultChain)],
                AzureEndpoint {
                    scheme: AzureScheme::Http,
                    addressing: Addressing::PathStyle,
                },
            )
        );

        // An empty entry is the same as no entry: anonymous, https, host-style.
        let empty: AzureEndpointOptions = toml::from_str("").unwrap();
        assert_eq!(empty, AzureEndpointOptions::default());
        assert_eq!(
            empty.fetch(Some(&container("releases"))),
            AzureFetchOptions::default()
        );
        assert!(!empty.fetch(Some(&container("releases"))).auth.is_granted());
        assert_eq!(empty.endpoint(), AzureEndpoint::default());
    }

    /// A grant applies to the container it names and to no other, which is the
    /// whole reason the table is keyed per container: one storage account holds
    /// private and anonymous-read containers side by side.
    #[test]
    fn a_grant_applies_to_one_container_only() {
        let opts: AzureEndpointOptions = toml::from_str(
            r#"
            [auth]
            releases = true
            public = false
            "#,
        )
        .unwrap();

        assert!(opts.fetch(Some(&container("releases"))).auth.is_granted());
        // An explicit `false` and an unlisted container behave identically; the
        // spelling exists so a reader can tell "deliberately unsigned" from
        // "forgotten", and so a higher-precedence file can revoke a grant.
        assert!(!opts.fetch(Some(&container("public"))).auth.is_granted());
        assert!(!opts.fetch(Some(&container("staging"))).auth.is_granted());

        // A URL naming no container has nothing to attribute a grant to.
        assert!(!opts.fetch(None).auth.is_granted());

        // `grants` reports what the file says, explicit `false` included — in the
        // order the document's table iterated (`toml::Table` is a `BTreeMap`, so
        // that is byte order, not write order).
        assert_eq!(
            opts.grants().collect::<Vec<_>>(),
            vec![
                (&container("public"), Auth::Anonymous),
                (&container("releases"), Auth::DefaultChain),
            ]
        );
    }

    /// The endpoint is host-scoped and replaces wholesale; the grants merge per
    /// container, in both directions — a higher file adds one grant without
    /// dropping another, and revokes with an explicit `false`.
    #[test]
    fn layering_replaces_the_endpoint_and_merges_the_grants() {
        let lower: AzureEndpointOptions = toml::from_str(
            r#"
            path-style = true

            [auth]
            releases = true
            staging = true
            "#,
        )
        .unwrap();
        let higher: AzureEndpointOptions = toml::from_str(
            r#"
            scheme = "http"

            [auth]
            staging = false
            internal = true
            "#,
        )
        .unwrap();

        let merged = higher.layered_over(&lower);

        assert_eq!(merged.endpoint(), higher.endpoint());
        assert!(
            merged.fetch(Some(&container("releases"))).auth.is_granted(),
            "a grant the higher file never mentions must survive"
        );
        assert!(
            !merged.fetch(Some(&container("staging"))).auth.is_granted(),
            "an explicit `false` in the higher file must revoke the lower grant"
        );
        assert!(merged.fetch(Some(&container("internal"))).auth.is_granted());
    }

    /// A container name Azure would refuse is a config error at load, not a grant
    /// that can never match anything.
    #[test]
    fn an_unusable_container_key_is_rejected() {
        let err = toml::from_str::<AzureEndpointOptions>("[auth]\nReleases = true\n")
            .expect_err("uppercase is not a legal container name");
        assert!(err.to_string().contains("Releases"), "{err}");
    }

    /// Round-tripping must preserve the boolean spelling, not leak the enum
    /// variant names into a written config file.
    #[test]
    fn enums_serialize_back_to_bools() {
        let toml = toml::to_string(&AzureEndpointOptions::new(
            [
                (container("releases"), Auth::DefaultChain),
                (container("public"), Auth::Anonymous),
            ],
            AzureEndpoint {
                scheme: AzureScheme::Http,
                addressing: Addressing::PathStyle,
            },
        ))
        .unwrap();
        assert!(toml.contains("releases = true"), "{toml}");
        assert!(toml.contains("public = false"), "{toml}");
        assert!(toml.contains("path-style = true"), "{toml}");
        assert!(toml.contains(r#"scheme = "http""#), "{toml}");
        assert!(!toml.contains("DefaultChain"), "{toml}");

        // An entry granting nothing writes no `auth` table at all, so a config
        // file keeps saying what it said.
        let anonymous = toml::to_string(&AzureEndpointOptions::default()).unwrap();
        assert!(!anonymous.contains("auth"), "{anonymous}");
    }
}
