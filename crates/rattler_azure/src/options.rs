//! Per-host endpoint options for Azure Blob channels.
//!
//! An entry in the `azure-options` config table is the *only* thing that grants
//! a host anything: without one, a channel on that host is fetched anonymously
//! over https in host-style addressing. There is deliberately no hardcoded list
//! of "official" Azure suffixes — since a grant must be written per host,
//! suffix classification carries no security weight, and the absence of the list
//! is what lets custom endpoints and the Azurite emulator work at all.
//!
//! # Three types, one table
//!
//! [`AzureEndpointOptions`] is the file format. Nothing consumes it directly:
//! the fetch path takes [`AzureFetchOptions`] and the write path takes
//! [`AzureEndpoint`], so `auth` cannot reach a caller that supplies its own
//! credential and [`Addressing`] cannot reach one that never derives an account.
//! A field a consumer would have to ignore reads as a guarantee, and the write
//! path ignoring `auth` looked exactly like a credential gate that was never there.
//!
//! # Why enums for what the config spells as bools
//!
//! The TOML surface stays `auth = true` / `path-style = true`, because that is
//! the ergonomic spelling and it keeps the table skimmable. Internally each is an
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

/// Whether credentials may attach to requests for a host.
///
/// Defaults to [`Auth::Anonymous`]: a host gets no credentials until a config
/// entry says otherwise. Serializes as the bool `auth` in `azure-options`.
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

    /// Run the standard Azure credential chain for this host and sign with what
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

/// What the fetch middleware needs to reach one Azure Blob host.
///
/// [`Addressing`] is absent rather than ignored: the fetch path forwards a path
/// and never derives an account name from it.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AzureFetchOptions {
    /// Whether credentials may be sent to this host.
    pub auth: Auth,

    /// The scheme `az://` is rewritten to for this host.
    pub scheme: AzureScheme,
}

/// One `azure-options` entry, as the config file spells it.
///
/// This is the serde surface and nothing else: the three TOML keys live here, and
/// each consumer takes the narrower view it can actually act on, via
/// [`Self::endpoint`] or [`Self::fetch`]. The fields are private so that view is
/// the only way in.
///
/// The default value is the no-entry behaviour: anonymous, https, host-style. A
/// host with no config entry behaves exactly as if it had a defaulted entry, so
/// callers can look up an absent host and fall back to `default()` rather than
/// branching on presence.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(rename_all = "kebab-case", default)
)]
pub struct AzureEndpointOptions {
    auth: Auth,

    scheme: AzureScheme,

    /// The field is named for what it holds, but the config key stays
    /// `path-style`: that is the spelling users have written, and the bool bridge
    /// is what the key means.
    #[cfg_attr(feature = "serde", serde(rename = "path-style", alias = "path_style"))]
    addressing: Addressing,
}

impl AzureEndpointOptions {
    /// Build an entry from a grant and the endpoint it applies to.
    pub fn new(auth: Auth, endpoint: AzureEndpoint) -> Self {
        Self {
            auth,
            scheme: endpoint.scheme,
            addressing: endpoint.addressing,
        }
    }

    /// How to address this host, for the write path.
    pub fn endpoint(self) -> AzureEndpoint {
        AzureEndpoint {
            scheme: self.scheme,
            addressing: self.addressing,
        }
    }

    /// The grant and wire scheme, for the fetch path.
    pub fn fetch(self) -> AzureFetchOptions {
        AzureFetchOptions {
            auth: self.auth,
            scheme: self.scheme,
        }
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    /// The whole point of the bool bridge: the TOML stays boolean while the code
    /// sees enums, and an absent field takes the safe default.
    #[test]
    fn toml_bools_map_to_enums() {
        let opts: AzureEndpointOptions = toml::from_str(
            r#"
            auth = true
            scheme = "http"
            path-style = true
            "#,
        )
        .unwrap();
        assert_eq!(
            opts,
            AzureEndpointOptions::new(
                Auth::DefaultChain,
                AzureEndpoint {
                    scheme: AzureScheme::Http,
                    addressing: Addressing::PathStyle,
                },
            )
        );

        // An empty entry is the same as no entry: anonymous, https, host-style.
        let empty: AzureEndpointOptions = toml::from_str("").unwrap();
        assert_eq!(empty, AzureEndpointOptions::default());
        assert_eq!(empty.fetch(), AzureFetchOptions::default());
        assert!(!empty.fetch().auth.is_granted());
        assert_eq!(empty.endpoint(), AzureEndpoint::default());

        // `auth = false` is spelled out explicitly by some users; it must not be
        // mistaken for a grant.
        let denied: AzureEndpointOptions = toml::from_str("auth = false").unwrap();
        assert!(!denied.fetch().auth.is_granted());
    }

    /// Round-tripping must preserve the boolean spelling, not leak the enum
    /// variant names into a written config file.
    #[test]
    fn enums_serialize_back_to_bools() {
        let toml = toml::to_string(&AzureEndpointOptions::new(
            Auth::DefaultChain,
            AzureEndpoint {
                scheme: AzureScheme::Http,
                addressing: Addressing::PathStyle,
            },
        ))
        .unwrap();
        assert!(toml.contains("auth = true"), "{toml}");
        assert!(toml.contains("path-style = true"), "{toml}");
        assert!(toml.contains(r#"scheme = "http""#), "{toml}");
        assert!(!toml.contains("DefaultChain"), "{toml}");
    }
}
