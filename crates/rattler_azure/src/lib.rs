//! Helpers for deriving Azure Blob coordinates from channel URLs and for minting
//! short-lived credentials for them.
//!
//! # Host model
//!
//! This crate does not police which hosts are legitimate Azure endpoints: the
//! host a channel URL names is taken to be the storage endpoint it says it is.
//! What is *granted* — credentials, wire scheme, addressing style — is declared in
//! [`options`] and never inferred from the host name: the wire scheme and the
//! addressing per host, and credentials per *container*, because that is the scope
//! Azure's own RBAC has. The default grant is [`Auth::Anonymous`], so naming a host
//! or a container in a URL by itself sends nothing to it. Nothing here signs or sends a request either — that lives
//! in `rattler_networking` — but two functions do handle a credential:
//! `azblob_config` embeds the account key or SAS it is handed into the config it
//! returns, and `mint_user_delegation_sas` spends the user's `az login` session to
//! obtain one. Deriving coordinates from a URL ([`account_and_container`])
//! touches no credential at all.
//!
//! Userinfo (`user:pass@host`) is rejected wherever a host is parsed, because
//! `az://real.host@evil.example/…` reads as the real host while addressing the
//! attacker's and provides no real functionality.

#[cfg(feature = "clap")]
pub mod clap;

pub mod options;

pub use options::{
    Addressing, Auth, AzureEndpoint, AzureEndpointOptions, AzureFetchOptions, AzureScheme,
};

pub use secrecy::{ExposeSecret, SecretString};
use url::Url;

/// Credentials for authenticating to Azure Blob storage.
///
/// Exactly one authentication method is carried, so the ambiguous "both a key
/// and a SAS token" and "neither" states are unrepresentable. The storage
/// account name, endpoint, and container are not stored here: they are derived
/// by the consumer from the channel URL together with the host's addressing
/// style (see [`account_and_container`]).
///
/// Both variants hold a [`SecretString`], so `Debug` redacts them, the bytes are
/// zeroized on drop, and every read is a visible `expose_secret()`. The type has
/// no `Serialize`/`Deserialize` either, so it cannot reach disk.
#[derive(Clone, Debug)]
pub enum AzureCredentials {
    /// A shared storage account key.
    AccountKey(SecretString),

    /// A shared access signature (SAS) token.
    SasToken(SecretString),
}

/// Strip a single leading `?` from a SAS token.
///
/// `--sas-token` may be supplied with or without a leading `?`, but a SAS minted
/// by [`mint_user_delegation_sas`] never has one. Normalizing at the single point
/// where a token is handed to opendal means both sources behave identically.
pub fn normalize_sas_token(token: &str) -> &str {
    token.strip_prefix('?').unwrap_or(token)
}

/// Errors that can occur while deriving Azure Blob coordinates from a channel
/// URL.
#[derive(Debug, thiserror::Error)]
pub enum AzureUrlError {
    /// The URL has no host component.
    #[error("no host in Azure blob URL")]
    NoHost,

    /// The URL carries userinfo (`user:pass@host`).
    #[error(
        "Azure blob URL must not contain userinfo (`user:pass@host`): the `user@host` form is a \
         host-spoofing vector that can disguise the real target host, and userinfo is invalid in \
         blob URLs"
    )]
    UserInfoNotAllowed,

    /// The text handed to [`AzureHost::parse`] is not a usable `host[:port]`.
    ///
    /// This is what a malformed `azure-options` key produces, so it quotes the
    /// text back and says what was expected instead.
    #[error("`{authority}` is not a valid Azure host: {reason}; expected `host` or `host:port`")]
    InvalidHostAuthority {
        /// The offending authority text.
        authority: String,
        /// Why it was rejected.
        reason: String,
    },

    /// Host-style addressing was requested but the host has no account label: it
    /// is an IP literal, or a domain with only one label.
    ///
    /// This is the error an Azurite or custom-endpoint user hits first, and the
    /// fix is a config line rather than a URL change, so the message names that
    /// line verbatim instead of leaving the user to discover `path-style`. The
    /// host is spelled the way [`AzureHost`] spells it, which is the way the
    /// config table is keyed — a key copied out of this message matches.
    #[error(
        "Azure blob URL host `{0}` is not a dotted domain of the form `<account>.blob.<suffix>`, \
         so its first label cannot be a storage account. Such a host needs path-style addressing, \
         where the storage account is the first path segment instead; that is not selectable from \
         configuration yet, and will be enabled by `[azure-options.\"{0}\"]` with \
         `path-style = true`"
    )]
    InvalidHost(String),

    /// The URL has no path segment to read the account from (path-style only).
    #[error("could not derive account name from Azure blob URL")]
    NoAccount,

    /// The URL has no container path segment.
    #[error("no container in Azure blob URL")]
    NoContainer,

    /// The derived account name is not a legal Azure storage account name.
    #[error(
        "`{0}` is not a valid Azure storage account name: account names are 3-24 characters of \
         lowercase letters and digits only"
    )]
    InvalidAccountName(String),

    /// The derived container name is not a legal Azure blob container name.
    #[error(
        "`{0}` is not a valid Azure blob container name: container names are 3-63 characters of \
         lowercase letters, digits and hyphens, must start and end with a letter or digit, and \
         must not contain consecutive hyphens"
    )]
    InvalidContainerName(String),

    /// The channel URL string could not be parsed.
    #[error("`{value}` is not a valid URL")]
    InvalidUrl {
        /// The offending input.
        value: String,
        /// The underlying parse error.
        #[source]
        source: url::ParseError,
    },

    /// The written path is not the path the URL Standard resolves it to.
    ///
    /// `..` segments — including percent-encoded ones — are resolved before any
    /// segment is validated, so a path that reads as one container (or, path-style,
    /// one account) can address another. The resolved form is quoted so the user
    /// can see where the URL would actually have gone.
    #[error(
        "Azure blob channel URL path `{written}` is not the path it resolves to, `{resolved}`; a \
         channel URL must name the location it addresses, so write `{resolved}` if that is the \
         location you mean"
    )]
    NonCanonicalPath {
        /// The path as written.
        written: String,
        /// The path it resolves to.
        resolved: String,
    },

    /// A path segment percent-decodes to bytes that are not UTF-8.
    ///
    /// Blob names are UTF-8, so there is nothing to send such a segment as. Decoding
    /// lossily would substitute U+FFFD and address a different blob than the URL
    /// names, silently and without an error at any layer.
    #[error(
        "Azure blob channel URL segment `{segment}` percent-decodes to bytes that are not UTF-8, \
         so it cannot name a blob"
    )]
    NonUtf8Path {
        /// The segment as written.
        segment: String,
        /// Where the decoded bytes stop being UTF-8.
        #[source]
        source: std::str::Utf8Error,
    },

    /// A path segment contains `%2F`, an encoded slash.
    ///
    /// One segment holding a slash and two segments are different blob paths, and
    /// the URL Standard does not resolve `%2F`, so whichever reading we picked would
    /// be a place the URL text does not say. Refusing keeps the written path and the
    /// blob path the same shape.
    #[error(
        "Azure blob channel URL segment `{0}` contains an encoded slash (`%2F`); write the path \
         separator as `/` if you mean a new segment"
    )]
    EncodedSlashInPath(String),

    /// The channel URL does not use the `az://` scheme.
    #[error(
        "Azure blob channel URL must use the `az://` scheme, e.g. \
         `az://<account>.blob.core.windows.net/<container>/...`: got `{0}`"
    )]
    InvalidScheme(String),
}

/// A storage account name that has passed Azure's naming rules: 3-24 characters
/// of lowercase letters and digits.
///
/// Those rules are the only thing that keeps option-shaped text (`--as-user`,
/// `-o`) out of the `az` argv in [`mint_user_delegation_sas`], so the mint takes
/// this type: the guarantee is then carried by what the function accepts rather
/// than by every call site remembering to derive its name through a validating
/// path. The inner `String` is private and [`Self::new`] is the only way to one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountName(String);

impl AccountName {
    /// Check a name against Azure's storage account naming rules.
    pub fn new(name: &str) -> Result<Self, AzureUrlError> {
        let valid = (3..=24).contains(&name.len())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        valid
            .then(|| Self(name.to_string()))
            .ok_or_else(|| AzureUrlError::InvalidAccountName(name.to_string()))
    }

    /// The validated name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A blob container name that has passed Azure's naming rules: 3-63 characters of
/// lowercase letters, digits and hyphens, with no leading or trailing hyphen and
/// no consecutive hyphens.
///
/// Exists for the same reason as [`AccountName`], and is what the container half
/// of the `az` argv is spelled as.
///
/// It is also the key of an `auth` table in `azure-options`, which is why it is
/// hashable and has the same string serde bridge [`AzureHost`] has: a grant is
/// written per container, and the name a grant is stored under must be the name a
/// lookup arrives with. Azure's rules do the normalizing for free — a container
/// name is lowercase by construction, so unlike a host there is only ever one
/// spelling of one container.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(try_from = "String", into = "String")
)]
pub struct ContainerName(String);

impl ContainerName {
    /// Check a name against Azure's blob container naming rules.
    pub fn new(name: &str) -> Result<Self, AzureUrlError> {
        let valid = (3..=63).contains(&name.len())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !name.starts_with('-')
            && !name.ends_with('-')
            && !name.contains("--");
        valid
            .then(|| Self(name.to_string()))
            .ok_or_else(|| AzureUrlError::InvalidContainerName(name.to_string()))
    }

    /// The validated name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContainerName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for ContainerName {
    type Err = AzureUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// The serde bridge for using a `ContainerName` as a map key: serde hands map keys
/// over as owned strings, so `serde(try_from = "String")` is what routes a written
/// `auth` key through [`ContainerName::new`] instead of storing it raw. A key Azure
/// would refuse is then a config error at load, not a grant that can never match.
impl TryFrom<String> for ContainerName {
    type Error = AzureUrlError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<ContainerName> for String {
    fn from(container: ContainerName) -> Self {
        container.0
    }
}

/// The storage account and container an Azure Blob channel URL resolves to.
///
/// The fields are public because their *types* are the invariant: a
/// `AzureCoordinates` cannot be assembled from unvalidated text, whoever builds
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureCoordinates {
    /// The storage account name — the first host label under
    /// [`Addressing::HostStyle`], the first path segment under
    /// [`Addressing::PathStyle`].
    pub account: AccountName,

    /// The blob container name — the first path segment under
    /// [`Addressing::HostStyle`], the second under [`Addressing::PathStyle`].
    pub container: ContainerName,
}

/// Derive the storage account name and container from an Azure Blob channel URL.
///
/// Where the account name lives is decided by `addressing`, which comes from the
/// host's `azure-options` entry — it is not guessable from the URL, because
/// `https://host/a/b` is a valid reading under both styles:
///
/// - [`Addressing::HostStyle`] (real Azure, the default): account = first label
///   of the host, container = first path segment. The host must be a domain with
///   at least two labels, so IP literals and single-label hosts fail with
///   [`AzureUrlError::InvalidHost`], whose message names the config line that
///   switches to path-style.
/// - [`Addressing::PathStyle`] (Azurite and other emulators): account = first
///   path segment, container = second. On a host under a known Azure Blob suffix
///   this is almost certainly a config mistake — the two styles then disagree
///   about which name is the account while producing identical request URLs, so
///   nothing fails until a mint asks for a delegation SAS on whatever the path
///   spelled. It is only a warning: the list is advisory, cannot cover a
///   proxy or a private endpoint, and choosing the addressing for a host remains
///   the user's call.
///
/// The host is otherwise trusted verbatim (see the [crate-level docs] for the
/// host model): an honest, arbitrary host is the caller's responsibility. The
/// derived account and container are additionally held to Azure's own naming
/// rules — under *both* addressing styles, since path-style takes the account
/// from user-controlled path text. Those rules reject an empty name, any
/// character outside `[a-z0-9-]`, and a leading `-`.
///
/// [crate-level docs]: crate
pub fn account_and_container(
    channel: &AzureChannelUrl,
    addressing: Addressing,
) -> Result<AzureCoordinates, AzureUrlError> {
    let host = channel.host();
    let container_segment =
        || segment(channel, addressing.container_segment()).ok_or(AzureUrlError::NoContainer);

    let (account, container) = match addressing {
        Addressing::HostStyle => {
            let account = host
                .account_label()
                .ok_or_else(|| AzureUrlError::InvalidHost(host.to_string()))?;
            (account, container_segment()?)
        }
        Addressing::PathStyle => {
            if host.is_known_azure_blob_endpoint() {
                tracing::warn!(
                    "`path-style = true` is set for `{host}`, which is a real Azure Blob endpoint \
                     addressed host-style: its storage account is `{}`, not the first path \
                     segment. Requests still come out identical, but anything that needs the \
                     account on its own — minting a user-delegation SAS, for one — will use the \
                     path segment instead. Remove `path-style = true` from \
                     `[azure-options.\"{host}\"]` unless you meant it",
                    host.account_label().unwrap_or("<none>"),
                );
            }
            (
                segment(channel, 0).ok_or(AzureUrlError::NoAccount)?,
                container_segment()?,
            )
        }
    };

    Ok(AzureCoordinates {
        account: AccountName::new(account)?,
        container: ContainerName::new(container)?,
    })
}

/// The container an Azure Blob URL names, when it names one.
///
/// This is the fetch path's half of [`account_and_container`]: a grant is written
/// per container, so the middleware needs the container and nothing else — no
/// account, which is what keeps a URL on a host that cannot carry an account label
/// (an IP literal read host-style) from failing here where it fetches happily
/// today.
///
/// The two answers it can give are deliberately different:
///
/// - `Ok(None)`: the URL has no container segment — the host root, or a path too
///   short to have one under this addressing. There is nothing to attribute a
///   grant to, so the caller sends nothing.
/// - `Err`: the segment is there but is not a name Azure allows for a container.
///   No legitimate blob request can land here, so this is a malformed endpoint
///   rather than an ungranted one, and saying so beats going quietly anonymous and
///   surfacing later as an unexplained 401.
pub fn container(
    channel: &AzureChannelUrl,
    addressing: Addressing,
) -> Result<Option<ContainerName>, AzureUrlError> {
    segment(channel, addressing.container_segment())
        .map(ContainerName::new)
        .transpose()
}

/// The `index`-th path segment, or `None` when it is missing or empty.
///
/// An empty segment is a missing one: no Azure name may be empty, so `//general`
/// has no first segment rather than an unnamed one.
fn segment(channel: &AzureChannelUrl, index: usize) -> Option<&str> {
    channel
        .path_segments()
        .nth(index)
        .filter(|segment| !segment.is_empty())
}

/// A normalized Azure Blob endpoint authority: a host, and its port when one is
/// written.
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
    /// Parse and normalize a bare `host[:port]` authority.
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
        // otherwise be accepted as `host` — a different endpoint from the one whose
        // port the user was in the middle of writing. Port 0 it keeps, and `wire()`
        // then hands out `https://host:0/…`, which no connection can be made to.
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

    /// The port exactly as the authority spells it, when it spells one.
    ///
    /// An IPv6 literal is bracketed, so a colon inside it is never a port
    /// delimiter — only a `]:port` suffix is.
    fn written_port(authority: &str) -> Option<&str> {
        let (_, port) = authority.rsplit_once(':')?;
        (!port.ends_with(']')).then_some(port)
    }

    /// Parse `<scheme>://<authority>`, reporting a failure against the authority
    /// text the caller actually wrote.
    fn parse_as(authority: &str, scheme: &str) -> Result<Url, AzureUrlError> {
        Url::parse(&format!("{scheme}://{authority}")).map_err(|err| {
            AzureUrlError::InvalidHostAuthority {
                authority: authority.to_string(),
                reason: err.to_string(),
            }
        })
    }

    /// Apply the rules the URL host parser does not: strip the DNS root label,
    /// reject empty labels, and hold the name to the 253-character limit DNS puts
    /// on one.
    ///
    /// Private, so every route in goes through [`parse`](Self::parse) and no rule
    /// can be skipped.
    fn normalized(
        host: url::Host,
        port: Option<u16>,
        authority: &str,
    ) -> Result<Self, AzureUrlError> {
        const DNS_NAME_LIMIT: usize = 253;

        let url::Host::Domain(domain) = &host else {
            // An IP literal is already fully canonical, and has no labels.
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

    /// The parsed host, without the port.
    pub fn host(&self) -> &url::Host {
        &self.host
    }

    /// The port, when the authority names one.
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// The storage account label under host-style addressing.
    ///
    /// `None` whenever the host cannot carry an account name, which the stored
    /// [`url::Host`] answers by construction rather than by inspecting text:
    ///
    /// - an IP literal is an [`url::Host::Ipv4`] or [`url::Host::Ipv6`], so it can
    ///   never be read as a label — including `127.0.0.1`, which a "does it
    ///   contain a dot" test would happily split into an account named `127`;
    /// - a domain must have at least two labels, so `localhost` is rejected.
    ///
    /// [`parse`](Self::parse) has already guaranteed no label is empty and there
    /// is no trailing dot, so "at least two labels" here means "at least two
    /// non-empty labels".
    fn account_label(&self) -> Option<&str> {
        match &self.host {
            url::Host::Domain(domain) => {
                let mut labels = domain.split('.');
                let first = labels.next()?;
                labels.next().is_some().then_some(first)
            }
            url::Host::Ipv4(_) | url::Host::Ipv6(_) => None,
        }
    }

    /// Whether this host sits under a suffix Microsoft operates, where the account
    /// is by definition the first label.
    ///
    /// Advisory only, and deliberately not a security boundary: a grant is written
    /// per host, so no behaviour hangs off this answer. It exists to warn about a
    /// `path-style = true` that cannot be what the user meant. A proxy or private
    /// endpoint in front of real Azure answers `false`, which is why a `false` here
    /// is never treated as evidence of anything.
    pub fn is_known_azure_blob_endpoint(&self) -> bool {
        const SUFFIXES: &[&str] = &[
            "blob.core.windows.net",       // global
            "blob.core.usgovcloudapi.net", // US Government
            "blob.core.chinacloudapi.cn",  // China, operated by 21Vianet
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
        // The canonical text *is* the identity; the host/port split is an
        // implementation detail, and printing it would only make a config dump
        // harder to read.
        write!(f, "AzureHost({:?})", self.to_string())
    }
}

impl std::str::FromStr for AzureHost {
    type Err = AzureUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// The serde bridge for using an `AzureHost` as a map key: serde hands map keys
/// over as owned strings, so `serde(try_from = "String")` is what routes a written
/// `azure-options` key through [`AzureHost::parse`] instead of storing it raw.
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

/// A validated Azure Blob **channel** URL, which has two spellings: `az://…` as
/// the user writes it and in configuration, and `http(s)://…` on the wire.
///
/// # Why the parts are stored, and not a URL
///
/// The obvious shape is a struct holding both spellings, which can hold a pair
/// that disagrees — a canonical URL for one host and a wire URL for another — and
/// nothing but discipline stops it. The next-obvious shape is one `Url` in the
/// wire form with a fixed scheme, deriving the other spelling from it. That is
/// worse than it looks: a `Url`'s port is scheme-relative, so storing
/// `az://host:443/…` as `https` drops the port on the way in, and
/// [`wire`](Self::wire) then hands out `http://host/…` — port 80, a different
/// endpoint.
///
/// So the authority is stored as an [`AzureHost`], which holds host and port
/// explicitly and normalizes both without reference to any scheme, next to the
/// already-normalized path, query and fragment. Every spelling is built from those
/// same parts by one private helper, so no two spellings can disagree about host,
/// port, path or query.
///
/// # Why the scheme is a `wire()` argument and not a field
///
/// Which scheme a host is reached over comes from its `azure-options` entry, and
/// [`parse`](Self::parse) runs as a clap `value_parser` — before any config file
/// is read. A stored scheme would therefore have to be a guess made at parse time
/// and corrected later, which is exactly the drift this type exists to prevent.
/// Passing it in at call time keeps the choice at the site that makes it.
///
/// Nothing in the type ties the argument to an options entry. `rattler-index`
/// takes it from the channel host's entry; `rattler_upload` passes the default,
/// because it reads no config file at all (see the note in
/// `rattler_upload::upload_from_args`).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AzureChannelUrl {
    /// The authority, normalized independently of any scheme.
    host: AzureHost,

    /// The path as the URL Standard normalizes it: always a leading `/`, still
    /// percent-encoded.
    path: String,

    /// The query, when there is one — a SAS token may be written inline.
    query: Option<String>,

    /// The fragment, when there is one.
    ///
    /// Kept so [`canonical`](Self::canonical) spells the channel back the way the
    /// user wrote it, which is also the spelling config keys are matched against.
    /// It reaches no server: an HTTP request carries only the path and query, and
    /// on a signed request it is gone from the URL as well, because
    /// `AzureMiddleware::sign` round-trips through `http::Uri`, which has no
    /// fragment.
    fragment: Option<String>,
}

impl AzureChannelUrl {
    /// Parse and validate an `az://` channel URL.
    ///
    /// The only accepted spelling is `az://<host>/<…>`. A bare `http(s)://` URL is
    /// deliberately *not* accepted: `az://` is the single canonical spelling for an
    /// Azure channel, and accepting the wire URL as a second input spelling would
    /// only invite confusion about which one is authoritative.
    ///
    /// Account and container derivation is *not* performed here: it depends on the
    /// host's addressing style, which is config that does not exist yet at clap
    /// parse time. It happens in [`account_and_container`], which today runs only
    /// where an account name is genuinely needed — minting a SAS from an
    /// `az login` session, and building the opendal config for a write. The fetch
    /// path never calls it.
    pub fn parse(value: &str) -> Result<Self, AzureUrlError> {
        // URL schemes are case-insensitive and `Url` lowercases them, so `AZ://…`
        // reaches every downstream `scheme() == "az"` comparison as `az`. Matching
        // case-insensitively here keeps this parser from rejecting what those
        // comparisons accept.
        let rest = strip_az_scheme(value)
            .ok_or_else(|| AzureUrlError::InvalidScheme(value.to_string()))?;

        // The authority runs to the first path, query or fragment delimiter. `\` is
        // in the set because the special-scheme parser used below treats it as `/`,
        // and splitting on it keeps the authority this type validates equal to the
        // authority that parser sees.
        let authority_end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(authority_end);
        let host = AzureHost::parse(authority)?;

        // Parse the whole thing as `https` for the path, query and fragment: the
        // special-scheme parser is what normalizes them, and `wire()` hands them
        // straight to an `http(s)` URL, so they have to be normalized its way.
        let url = Url::parse(&format!("https://{authority}{tail}")).map_err(|source| {
            AzureUrlError::InvalidUrl {
                value: value.to_string(),
                source,
            }
        })?;

        // Dot segments — `%2e%2e` as much as `..` — are resolved by that parser
        // before anything here has looked at a segment, so a path reading as one
        // container (path-style: one *account*) can address another. Nothing needs
        // to guess which rewrites are benign: a path that is not already the path
        // it resolves to is not the path the user can be assumed to have meant.
        let written = match tail.split(['?', '#']).next().unwrap_or_default() {
            "" => "/",
            path => path,
        };
        if written != url.path() {
            return Err(AzureUrlError::NonCanonicalPath {
                written: written.to_string(),
                resolved: url.path().to_string(),
            });
        }

        // Every segment must survive the round trip to a blob name. `%2F` is checked
        // before decoding, because after it there is no telling it from a `/` the
        // user wrote.
        for segment in url.path_segments().into_iter().flatten() {
            if segment.to_ascii_uppercase().contains("%2F") {
                return Err(AzureUrlError::EncodedSlashInPath(segment.to_string()));
            }
            percent_encoding::percent_decode_str(segment)
                .decode_utf8()
                .map_err(|source| AzureUrlError::NonUtf8Path {
                    segment: segment.to_string(),
                    source,
                })?;
        }

        Ok(Self {
            host,
            path: url.path().to_string(),
            query: url.query().map(str::to_string),
            fragment: url.fragment().map(str::to_string),
        })
    }

    /// The `az://host/path` spelling: the channel's identity.
    ///
    /// This is what users write, what is shown back to them, and what config keys
    /// are matched against: `rattler-index` resolves `[index-config."az://…"]`
    /// through this spelling, and `[azure-options."…"]` through [`Self::host`].
    /// Matching the wire string instead was reviewer issue 5 — the two spellings
    /// exist so a config key never has to guess which one a channel was stored as.
    /// A SAS written inline is masked: this spelling is the one that reaches logs
    /// and error messages, and [`Self::wire`] is the only way to the signature.
    pub fn canonical(&self) -> Url {
        self.spelled("az", Sas::Masked)
    }

    /// The `http(s)://host/path` spelling used for actual requests, over the
    /// scheme the host's options entry asks for.
    pub fn wire(&self, scheme: AzureScheme) -> Url {
        self.spelled(scheme.as_str(), Sas::Exposed)
    }

    /// Build one spelling of this URL.
    ///
    /// Both public spellings go through here, so they cannot differ in anything
    /// but the scheme and whether the signature is masked: the host, port, path,
    /// query and fragment they are built from are literally the same values.
    fn spelled(&self, scheme: &str, sas: Sas) -> Url {
        let mut text = format!("{scheme}://{}{}", self.host, self.path);
        if let Some(query) = &self.query {
            text.push('?');
            match sas {
                Sas::Exposed => text.push_str(query),
                Sas::Masked => text.push_str(&mask_sas_signature(query)),
            }
        }
        if let Some(fragment) = &self.fragment {
            text.push('#');
            text.push_str(fragment);
        }
        // Cannot fail: the authority re-serializes to the normalized form it was
        // parsed from, and the path, query and fragment are already-encoded output
        // of a `Url` parse. Every host shape `AzureHost` can hold (normalized
        // domain, IPv4 literal, bracketed IPv6) is valid both to the special-scheme
        // host parser and to the opaque-host parser `az://` gets.
        Url::parse(&text).expect("a normalized authority, path and query is a valid URL")
    }

    /// The host, with its port when the URL carries one.
    ///
    /// This is the `azure-options` key for the channel, so options can be looked
    /// up without a caller re-deriving it from a URL and getting the port handling
    /// subtly wrong.
    pub fn host(&self) -> &AzureHost {
        &self.host
    }

    /// The still-encoded path segments, exactly as [`Url::path_segments`] would
    /// yield them for the wire spelling.
    fn path_segments(&self) -> std::str::Split<'_, char> {
        self.path.strip_prefix('/').unwrap_or(&self.path).split('/')
    }
}

impl std::fmt::Display for AzureChannelUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The canonical spelling is the one users recognize and the one config is
        // keyed by, so it is the only sensible thing to print.
        write!(f, "{}", self.canonical())
    }
}

impl std::fmt::Debug for AzureChannelUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Derived, this would print the raw query and hand a `{:?}` on any struct
        // holding a channel the signature that `canonical()` exists to withhold.
        f.debug_tuple("AzureChannelUrl")
            .field(&self.canonical().as_str())
            .finish()
    }
}

/// Whether a spelling of a channel URL may carry the SAS signature.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sas {
    /// For the wire: the signature is what makes the request authentic.
    Exposed,
    /// For anything a human or a log sees.
    Masked,
}

/// Replace the value of a query's `sig` parameter, leaving the rest intact.
///
/// The other SAS parameters (`sv`, `se`, `sp`, …) describe the grant and are worth
/// showing; `sig` is the secret that makes it usable.
fn mask_sas_signature(query: &str) -> String {
    query
        .split('&')
        .map(|parameter| match parameter.split_once('=') {
            Some((name, _)) if name.eq_ignore_ascii_case("sig") => format!("{name}=REDACTED"),
            _ => parameter.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

impl std::str::FromStr for AzureChannelUrl {
    type Err = AzureUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Strip a case-insensitive `az://` prefix, or return `None` when it is absent.
fn strip_az_scheme(value: &str) -> Option<&str> {
    const PREFIX: &str = "az://";
    // `get` rather than slicing: a multi-byte leading character would panic on a
    // non-char-boundary index.
    value
        .get(..PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
        .map(|_| &value[PREFIX.len()..])
}

/// Build an opendal [`AzblobConfig`](opendal::services::AzblobConfig) from a
/// channel URL, the endpoint options of its host, and credentials.
///
/// The account name, endpoint, container and root prefix are all derived from the
/// channel URL, read the way `options.addressing` says to read it and reached over
/// `options.scheme`; the credentials supply only the account key or SAS token.
/// The per-container grants are not part of [`AzureEndpoint`] at all — this is the
/// write path, where the credential has already been chosen by the caller.
///
/// Taking the [`AzureChannelUrl`] rather than a wire `Url` is what keeps the
/// scheme in the config from disagreeing with the scheme in the endpoint: both
/// come from the same `options`.
///
/// # The two addressing shapes
///
/// opendal's azblob core builds every request URI as `{endpoint}/{container}/{path}`
/// and its core struct carries no account field at all, so under path-style the
/// account can only reach the URL through `endpoint`:
///
/// - [`Addressing::HostStyle`]: `endpoint` is `{scheme}://{host}[:{port}]`, the
///   account is the first host label, and `root` is the path after the container.
/// - [`Addressing::PathStyle`]: `endpoint` is
///   `{scheme}://{host}[:{port}]/{account}`, the account is the first path
///   segment, and `root` is the path after *both* the account and the container.
///
/// `account_name` is set under both styles, and is mandatory under both: opendal
/// infers it only from three known Azure suffixes and returns `None` — not an
/// error — for anything else, so omitting it from a path-style config makes
/// shared-key signing quietly never engage, and the failure surfaces as a 403
/// rather than as a config error.
///
/// Neither endpoint ends in a slash. `AzblobBuilder::endpoint` trims one, but this
/// builds the config struct literally, where nothing does, and a stray slash would
/// yield `//{container}/…`.
#[cfg(feature = "opendal")]
pub fn azblob_config(
    credentials: &AzureCredentials,
    channel: &AzureChannelUrl,
    endpoint_options: AzureEndpoint,
) -> Result<opendal::services::AzblobConfig, AzureUrlError> {
    let AzureCoordinates { account, container } =
        account_and_container(channel, endpoint_options.addressing)?;

    // The authority comes from `AzureHost`, not from a wire URL: a `Url` has
    // already dropped a port equal to its scheme's default, so reading it back
    // would turn a written `:443` into no port at all.
    let authority = channel.host();
    let endpoint = match endpoint_options.addressing {
        Addressing::HostStyle => format!("{}://{authority}", endpoint_options.scheme),
        Addressing::PathStyle => format!("{}://{authority}/{account}", endpoint_options.scheme),
    };

    // Root prefix = the path after the segments the coordinates already consumed:
    // the container, plus the account when path-style put it in the path. Skipping
    // one too few there leaves the account segment inside `root`, so every blob is
    // written one directory deeper than the channel actually lives — silently, and
    // in the right container, which is what makes it hard to spot.
    let consumed = match endpoint_options.addressing {
        Addressing::HostStyle => 1,
        Addressing::PathStyle => 2,
    };

    // Percent-decode each segment: `path_segments()` yields still-encoded segments
    // and opendal percent-encodes `root + path` again, so passing them through
    // verbatim would double-encode a prefix containing a space or a `+`.
    // `account_and_container` has already confirmed the consumed segments exist.
    let root = format!(
        "/{}",
        channel
            .path_segments()
            .skip(consumed)
            // Infallible in practice: `AzureChannelUrl::parse` rejects a segment that
            // does not decode to UTF-8. Erroring rather than substituting U+FFFD is
            // what keeps that a guarantee instead of an assumption.
            .map(|segment| {
                percent_encoding::percent_decode_str(segment)
                    .decode_utf8()
                    .map_err(|source| AzureUrlError::NonUtf8Path {
                        segment: segment.to_string(),
                        source,
                    })
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("/")
    );

    let (account_key, sas_token) = match credentials {
        AzureCredentials::AccountKey(key) => (Some(key.expose_secret().to_string()), None),
        AzureCredentials::SasToken(token) => (
            None,
            Some(normalize_sas_token(token.expose_secret()).to_string()),
        ),
    };

    Ok(opendal::services::AzblobConfig {
        endpoint: Some(endpoint),
        account_name: Some(account.as_str().to_string()),
        container: container.as_str().to_string(),
        root: Some(root),
        account_key,
        sas_token,
        ..Default::default()
    })
}

/// Errors that can occur while minting a user-delegation SAS via the Azure CLI.
#[cfg(feature = "clap")]
#[derive(Debug, thiserror::Error)]
pub enum AzureCliSasError {
    /// The SAS expiry timestamp could not be computed.
    #[error("failed to compute the SAS expiry timestamp: {0}")]
    Expiry(String),

    /// The `az` executable could not be resolved on `PATH`.
    #[error("could not resolve the Azure CLI (`az`) on PATH; install it and run `az login`")]
    AzResolve(#[source] which::Error),

    /// The `az` process could not be spawned.
    #[error("failed to run the Azure CLI (`az`)")]
    Spawn(#[source] std::io::Error),

    /// `az` exited with a non-zero status.
    #[error("the Azure CLI failed to generate a user-delegation SAS (is `az login` current?): {0}")]
    CommandFailed(String),

    /// `az` succeeded but produced no SAS token.
    #[error("the Azure CLI returned an empty SAS token")]
    EmptyOutput,
}

/// Mint a short-lived user-delegation SAS for a container by shelling out to the
/// Azure CLI.
///
/// opendal's azblob backend (used by the index and upload write paths) only
/// accepts a shared account key or a SAS token, not an AAD bearer token. To let
/// users authenticate writes with their `az login` session, this converts that
/// session into a SAS via:
///
/// ```text
/// az storage container generate-sas --account-name <account> --name <container>
///     --permissions <permissions> --expiry <expiry> --auth-mode login --as-user
///     [--https-only] -o tsv
/// ```
///
/// `permissions` is the Azure SAS permission string (e.g. `"cw"`). The returned
/// token has no leading `?`. Requires `az` on `PATH` and a prior `az login`.
///
/// `scheme` is the wire scheme the host's options entry asks for: `--https-only`
/// restricts the SAS to TLS, which would make it unusable against a host reached
/// over http.
///
/// Runs the `az` process on the tokio runtime; it is meant to be called once at
/// setup time.
///
/// # Container-scope limitation
///
/// A user-delegation SAS minted against a flat container is *container-scoped*,
/// not prefix-scoped: it grants its permissions over the whole container, so a
/// SAS for one channel also grants rights over any sibling channels that share
/// the same container. The short TTL requested here bounds the blast radius, but
/// prefix-scoping a flat container is not possible without a stored access
/// policy, which this path deliberately does not create.
#[cfg(feature = "clap")]
pub async fn mint_user_delegation_sas(
    account: &AccountName,
    container: &ContainerName,
    permissions: &str,
    valid_for: std::time::Duration,
    scheme: AzureScheme,
) -> Result<SecretString, AzureCliSasError> {
    /// Slack for a client clock running up to two minutes slow, since the expiry
    /// is computed here and evaluated by Azure.
    const CLOCK_SKEW_HEADROOM: std::time::Duration = std::time::Duration::from_secs(120);

    let signed = jiff::SignedDuration::try_from(valid_for.saturating_add(CLOCK_SKEW_HEADROOM))
        .map_err(|err| AzureCliSasError::Expiry(err.to_string()))?;
    let expiry = jiff::Timestamp::now()
        .checked_add(signed)
        .map_err(|err| AzureCliSasError::Expiry(err.to_string()))?;
    // `az` expects an ISO-8601 UTC timestamp; keep second precision so the window
    // is not floored down to the enclosing whole minute.
    let expiry = expiry.strftime("%Y-%m-%dT%H:%M:%SZ").to_string();

    let mut command = az_command()?;

    let output = command
        .args(generate_sas_args(
            account,
            container,
            permissions,
            &expiry,
            scheme,
        ))
        .output()
        .await
        .map_err(AzureCliSasError::Spawn)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AzureCliSasError::CommandFailed(stderr));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(AzureCliSasError::EmptyOutput);
    }
    Ok(token.into())
}

/// The argv for the `az storage container generate-sas` call.
///
/// Split out from the spawn so the argument list is testable without an `az` on
/// `PATH`. It stays a list of separate arguments — never a command line — so no
/// value can be read as anything but one argument.
#[cfg(feature = "clap")]
fn generate_sas_args<'a>(
    account: &'a AccountName,
    container: &'a ContainerName,
    permissions: &'a str,
    expiry: &'a str,
    scheme: AzureScheme,
) -> Vec<&'a str> {
    let mut args = vec![
        "storage",
        "container",
        "generate-sas",
        "--account-name",
        account.as_str(),
        "--name",
        container.as_str(),
        "--permissions",
        permissions,
        "--expiry",
        expiry,
        "--auth-mode",
        "login",
        "--as-user",
    ];
    if let AzureScheme::Https = scheme {
        args.push("--https-only");
    }
    args.extend(["-o", "tsv"]);
    args
}

/// Build the [`tokio::process::Command`] used to invoke the Azure CLI.
///
/// `which` resolves `az` up front so a missing CLI surfaces as [`AzureCliSasError::AzResolve`]
/// rather than an opaque spawn failure. It also matters on Windows, where the CLI
/// is an `az.cmd` batch shim: the process spawner does not honor `PATHEXT`, so a
/// bare `az` fails to resolve, but `which` applies `PATHEXT` to find the real path.
/// The resolved path is invoked directly; routing through the command interpreter
/// (`cmd /C az ...`) is deliberately avoided as an argument-injection vector.
#[cfg(feature = "clap")]
fn az_command() -> Result<tokio::process::Command, AzureCliSasError> {
    let path = which::which("az").map_err(AzureCliSasError::AzResolve)?;
    Ok(tokio::process::Command::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every derivation runs off an [`AzureChannelUrl`], so the tests build one.
    fn channel(url: &str) -> AzureChannelUrl {
        AzureChannelUrl::parse(url).unwrap_or_else(|err| panic!("{url} should parse: {err}"))
    }

    fn coordinates(account: &str, container: &str) -> AzureCoordinates {
        AzureCoordinates {
            account: AccountName::new(account).expect("test account name"),
            container: ContainerName::new(container).expect("test container name"),
        }
    }

    #[test]
    fn normal_url_resolves() {
        assert_eq!(
            account_and_container(
                &channel("az://acct.blob.core.windows.net/general/noarch"),
                Addressing::HostStyle
            )
            .unwrap(),
            coordinates("acct", "general")
        );
    }

    /// The fetch path's derivation: it must find the same container
    /// `account_and_container` does, under both addressing styles, and it must not
    /// inherit that function's account rules — a host-style IP literal has no
    /// account label, but it still has a container, and a fetch for it is a request
    /// that works today.
    #[test]
    fn container_is_derived_from_the_addressing() {
        for (url, addressing, expected) in [
            (
                "az://acct.blob.core.windows.net/general/noarch",
                Addressing::HostStyle,
                "general",
            ),
            (
                "az://127.0.0.1:10000/devstoreaccount1/general/noarch",
                Addressing::PathStyle,
                "general",
            ),
            (
                "az://127.0.0.1:10000/general/noarch",
                Addressing::HostStyle,
                "general",
            ),
        ] {
            assert_eq!(
                container(&channel(url), addressing).unwrap(),
                Some(ContainerName::new(expected).unwrap()),
                "{url}"
            );
        }

        // Where both derivations answer, they must answer the same thing: a grant
        // looked up for one container and applied to another is a security bug.
        for (url, addressing) in [
            (
                "az://acct.blob.core.windows.net/general/noarch",
                Addressing::HostStyle,
            ),
            (
                "az://127.0.0.1:10000/devstoreaccount1/general",
                Addressing::PathStyle,
            ),
        ] {
            assert_eq!(
                container(&channel(url), addressing).unwrap(),
                Some(
                    account_and_container(&channel(url), addressing)
                        .unwrap()
                        .container
                ),
                "{url}"
            );
        }
    }

    /// A URL with no container segment is not an error: there is nothing to
    /// attribute a grant to, so the fetch path sends nothing and stays total for
    /// URLs that are not channel-scoped.
    #[test]
    fn a_url_without_a_container_names_none() {
        for (url, addressing) in [
            ("az://acct.blob.core.windows.net", Addressing::HostStyle),
            ("az://acct.blob.core.windows.net/", Addressing::HostStyle),
            (
                "az://127.0.0.1:10000/devstoreaccount1",
                Addressing::PathStyle,
            ),
            ("az://127.0.0.1:10000/", Addressing::PathStyle),
        ] {
            assert_eq!(container(&channel(url), addressing).unwrap(), None, "{url}");
        }
    }

    /// A segment that cannot be a container name is a malformed endpoint, not an
    /// ungranted one — Azure forbids uppercase, so no legitimate request lands
    /// here. Going quietly anonymous would surface as an unexplained 401 instead of
    /// naming the fault.
    #[test]
    fn a_url_with_an_unusable_container_is_an_error() {
        for url in [
            "az://acct.blob.core.windows.net/General/noarch",
            "az://acct.blob.core.windows.net/ab/noarch",
            "az://acct.blob.core.windows.net/a--b/noarch",
        ] {
            let err = container(&channel(url), Addressing::HostStyle)
                .expect_err("an illegal container name must be reported");
            assert!(
                matches!(err, AzureUrlError::InvalidContainerName(_)),
                "{url}: {err}"
            );
        }
    }

    #[test]
    fn userinfo_is_rejected() {
        assert!(matches!(
            AzureChannelUrl::parse("az://acct.blob.core.windows.net@evil.example/general"),
            Err(AzureUrlError::UserInfoNotAllowed)
        ));
        assert!(matches!(
            AzureHost::parse("acct.blob.core.windows.net@evil.example"),
            Err(AzureUrlError::UserInfoNotAllowed)
        ));
    }

    /// Azure's naming rules are what keep injection-shaped values out of the `az`
    /// subprocess, so they have to hold under path-style too — where the account
    /// comes from user-controlled path text rather than a host label.
    #[test]
    fn invalid_component_names_are_rejected_under_both_styles() {
        assert!(matches!(
            account_and_container(
                &channel("az://acct.blob.core.windows.net/general;evil/noarch"),
                Addressing::HostStyle
            ),
            Err(AzureUrlError::InvalidContainerName(_))
        ));

        for (path, account_at_fault) in [
            ("az://127.0.0.1:10000/devstore;evil/general", true),
            ("az://127.0.0.1:10000/DevStoreAccount1/general", true),
            // Azure allows no hyphen at all in an account name.
            ("az://127.0.0.1:10000/dev-store/general", true),
            // Too short for Azure, whatever the charset says.
            ("az://127.0.0.1:10000/ab/general", true),
            ("az://127.0.0.1:10000/devstoreaccount1/general;evil", false),
            ("az://127.0.0.1:10000/devstoreaccount1/ab", false),
            ("az://127.0.0.1:10000/devstoreaccount1/a--b", false),
            ("az://127.0.0.1:10000/devstoreaccount1/-general", false),
            ("az://127.0.0.1:10000/devstoreaccount1/general-", false),
        ] {
            let Err(err) = account_and_container(&channel(path), Addressing::PathStyle) else {
                panic!("expected a rejection for {path}");
            };
            let matched = if account_at_fault {
                matches!(err, AzureUrlError::InvalidAccountName(_))
            } else {
                matches!(err, AzureUrlError::InvalidContainerName(_))
            };
            assert!(matched, "unexpected error for {path}: {err}");
        }
    }

    /// The docstring on [`account_and_container`] promises option-shaped values can
    /// never reach the `az` argv. A charset of `[a-z0-9-]` alone does not deliver
    /// that, because a leading `-` is inside it.
    #[test]
    fn option_shaped_components_are_rejected() {
        for (url, account_at_fault) in [
            ("az://--as-user.blob.core.windows.net/general", true),
            ("az://-o.blob.core.windows.net/general", true),
            ("az://acct.blob.core.windows.net/--https-only/noarch", false),
            ("az://acct.blob.core.windows.net/-o/noarch", false),
        ] {
            let Err(err) = account_and_container(&channel(url), Addressing::HostStyle) else {
                panic!("expected a rejection for {url}");
            };
            let matched = if account_at_fault {
                matches!(err, AzureUrlError::InvalidAccountName(_))
            } else {
                matches!(err, AzureUrlError::InvalidContainerName(_))
            };
            assert!(matched, "unexpected error for {url}: {err}");
        }
    }

    /// An empty name reaching a constructor must be rejected there, not only by a
    /// caller remembering to filter it out first.
    #[test]
    fn empty_components_are_rejected() {
        assert!(AccountName::new("").is_err());
        assert!(ContainerName::new("").is_err());
    }

    #[test]
    fn path_style_derives_account_from_first_segment() {
        for host in [
            "127.0.0.1:10000",
            "[::1]:10000",
            "azurite:10000",
            "localhost:10000",
            // A bare docker service name, which is the shape a compose file gives.
            "azurite",
            "localhost",
        ] {
            assert_eq!(
                account_and_container(
                    &channel(&format!("az://{host}/devstoreaccount1/general/noarch")),
                    Addressing::PathStyle
                )
                .unwrap(),
                coordinates("devstoreaccount1", "general"),
                "path-style derivation failed for {host}"
            );
        }
    }

    #[test]
    fn path_style_needs_two_segments() {
        assert!(matches!(
            account_and_container(
                &channel("az://127.0.0.1:10000/devstoreaccount1"),
                Addressing::PathStyle
            ),
            Err(AzureUrlError::NoContainer)
        ));

        for empty in ["az://127.0.0.1:10000/", "az://127.0.0.1:10000"] {
            assert!(matches!(
                account_and_container(&channel(empty), Addressing::PathStyle),
                Err(AzureUrlError::NoAccount)
            ));
        }
    }

    /// A `path-style = true` entry on a real Azure host is a config mistake with no
    /// visible symptom — request URLs come out identical under both styles — right
    /// up to a mint asking for a delegation SAS on the account name the *path*
    /// happened to spell. Which addressing a host uses is still the user's call, so
    /// this warns and proceeds.
    #[test]
    #[tracing_test::traced_test]
    fn path_style_on_a_real_azure_host_warns_and_proceeds() {
        let coords = account_and_container(
            &channel("az://acct.blob.core.windows.net/general/mychannel"),
            Addressing::PathStyle,
        )
        .expect("addressing is the user's call, so this is a warning and not an error");
        assert_eq!(coords.account.as_str(), "general");

        assert!(logs_contain("path-style = true"));
        assert!(logs_contain("acct.blob.core.windows.net"));

        // A host that is not a known Azure endpoint gets no warning, however much
        // its first label looks like an account name.
        let coords = account_and_container(
            &channel("az://acct.blob.example.com/devstoreaccount1/general"),
            Addressing::PathStyle,
        )
        .unwrap();
        assert_eq!(coords.account.as_str(), "devstoreaccount1");
        assert!(!logs_contain("acct.blob.example.com"));
    }

    /// The suffix list is advisory, but a sloppy match on it would warn about
    /// hosts Microsoft does not operate — and stay silent on ones it does.
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

    /// Host-style must keep rejecting hosts it cannot derive an account from — and
    /// the rejection must hand the user a config key that would actually match,
    /// which means the port has to be in it and the host has to be spelled the way
    /// [`AzureHost`] spells it.
    #[test]
    fn host_style_rejects_undottable_hosts_with_a_guided_error() {
        for (host, expected_key) in [
            ("127.0.0.1:10000", "127.0.0.1:10000"),
            ("azurite:10000", "azurite:10000"),
            ("localhost", "localhost"),
            ("[::1]:10000", "[::1]:10000"),
            // A trailing dot is the DNS root label, not a second label: this host
            // must not sneak past the dotted-domain gate.
            ("localhost.", "localhost"),
            ("LocalHost", "localhost"),
            // A port equal to a wire scheme's default: it is part of the key, and
            // only survives because the channel — not a `Url` — is what is read.
            ("azurite:443", "azurite:443"),
            ("azurite:80", "azurite:80"),
        ] {
            let channel = channel(&format!("az://{host}/devstoreaccount1/general"));
            let err = account_and_container(&channel, Addressing::HostStyle)
                .expect_err("host-style must not accept an undottable host");
            assert!(matches!(err, AzureUrlError::InvalidHost(_)), "{err}");

            let message = err.to_string();
            assert!(message.contains("path-style = true"), "{message}");
            let key = format!("[azure-options.\"{expected_key}\"]");
            assert!(message.contains(&key), "{message}");
            // The key named must be the one an `azure-options` lookup is made
            // with, or the entry the user writes cannot ever apply.
            assert_eq!(expected_key, channel.host().to_string(), "{host}");
        }
    }

    /// An empty host label is never legal, and used to yield an empty first
    /// "label" as the account name.
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
                    AzureChannelUrl::parse(&format!("az://{host}/general/noarch")),
                    Err(AzureUrlError::InvalidHostAuthority { .. })
                ),
                "expected a rejection for {host}"
            );
        }
    }

    #[test]
    fn parse_requires_the_az_scheme() {
        for input in [
            "https://acct.blob.core.windows.net/general",
            "http://acct.blob.core.windows.net/general",
            "ftp://acct.blob.core.windows.net/general",
            "acct.blob.core.windows.net/general",
        ] {
            assert!(
                matches!(
                    AzureChannelUrl::parse(input),
                    Err(AzureUrlError::InvalidScheme(_))
                ),
                "expected InvalidScheme for {input}"
            );
        }
    }

    /// URL schemes are case-insensitive, and the middleware's `scheme() == "az"`
    /// test sees an already-lowercased scheme, so it accepts `AZ://`. This parser
    /// must not disagree with it.
    #[test]
    fn parse_accepts_a_scheme_in_any_case() {
        for input in [
            "AZ://acct.blob.core.windows.net/general",
            "Az://acct.blob.core.windows.net/general",
            "aZ://acct.blob.core.windows.net/general",
        ] {
            let channel = AzureChannelUrl::parse(input)
                .unwrap_or_else(|err| panic!("{input} should parse: {err}"));
            assert_eq!(
                channel.canonical().as_str(),
                "az://acct.blob.core.windows.net/general"
            );
        }
    }

    #[test]
    fn canonical_and_wire_round_trip() {
        let channel =
            AzureChannelUrl::parse("az://acct.blob.core.windows.net/general/noarch").unwrap();

        assert_eq!(
            channel.canonical().as_str(),
            "az://acct.blob.core.windows.net/general/noarch"
        );
        assert_eq!(
            channel.wire(AzureScheme::Https).as_str(),
            "https://acct.blob.core.windows.net/general/noarch"
        );
        assert_eq!(
            channel.wire(AzureScheme::Http).as_str(),
            "http://acct.blob.core.windows.net/general/noarch"
        );
        assert_eq!(channel.to_string(), channel.canonical().to_string());
        // `FromStr` is the same parser, so the canonical spelling parses back to
        // the same value — which is what lets a config key round-trip.
        assert_eq!(
            channel,
            channel
                .canonical()
                .as_str()
                .parse::<AzureChannelUrl>()
                .unwrap()
        );
    }

    /// The point of storing the parts: no scheme choice can make the two spellings
    /// describe different locations.
    #[test]
    fn spellings_cannot_disagree() {
        for input in [
            "az://acct.blob.core.windows.net/general/noarch",
            "az://127.0.0.1:10000/devstoreaccount1/general",
            "az://acct.blob.core.windows.net/general/with%20space?sv=token",
            // An IPv6 literal is the host shape most likely to break the canonical
            // rebuild, since it has to survive being re-parsed as an opaque host.
            "az://[::1]:10000/devstoreaccount1/general",
            // The scheme-default ports: exactly the spellings a `Url` stored with a
            // fixed scheme silently drops.
            "az://azurite.local:443/devstoreaccount1/general",
            "az://azurite.local:80/devstoreaccount1/general",
        ] {
            let channel = AzureChannelUrl::parse(input).unwrap();
            let canonical = channel.canonical();
            for scheme in [AzureScheme::Https, AzureScheme::Http] {
                let wire = channel.wire(scheme);
                assert_eq!(wire.scheme(), scheme.as_str());
                assert_eq!(canonical.host_str(), wire.host_str(), "{input}");
                assert_eq!(canonical.path(), wire.path(), "{input}");
                assert_eq!(canonical.query(), wire.query(), "{input}");

                // Ports are compared semantically, not textually: `az` has no
                // default port so the canonical form always spells one out when the
                // URL has one, while a wire URL omits a port equal to its scheme's
                // default. An omitted port on `http` *is* 80, so those agree.
                let default = match scheme {
                    AzureScheme::Https => 443,
                    AzureScheme::Http => 80,
                };
                assert_eq!(
                    wire.port_or_known_default(),
                    Some(canonical.port().unwrap_or(default)),
                    "{input} over {scheme}"
                );
            }
        }
    }

    /// The `:443` regression: a wire URL stored with the `https` scheme drops this
    /// port, and `wire(Http)` then names a completely different endpoint.
    #[test]
    fn a_written_default_port_survives() {
        let channel =
            AzureChannelUrl::parse("az://azurite.local:443/devstoreaccount1/general").unwrap();

        assert_eq!(channel.host().to_string(), "azurite.local:443");
        assert_eq!(channel.host().port(), Some(443));
        assert_eq!(
            channel.canonical().as_str(),
            "az://azurite.local:443/devstoreaccount1/general"
        );
        assert_eq!(
            channel.wire(AzureScheme::Http).as_str(),
            "http://azurite.local:443/devstoreaccount1/general"
        );
        assert_eq!(
            channel.wire(AzureScheme::Https).as_str(),
            "https://azurite.local/devstoreaccount1/general"
        );

        // Identity must not be scheme-relative either: a host on 443 is not the
        // same endpoint as the same host with no port, because the scheme that
        // would make them equal is not known here.
        let no_port =
            AzureChannelUrl::parse("az://azurite.local/devstoreaccount1/general").unwrap();
        assert_ne!(channel, no_port);
        assert_ne!(channel.host(), no_port.host());
    }

    #[test]
    fn host_keeps_a_non_default_port() {
        let emulator =
            AzureChannelUrl::parse("az://127.0.0.1:10000/devstoreaccount1/general").unwrap();
        assert_eq!(emulator.host().to_string(), "127.0.0.1:10000");
        assert_eq!(
            emulator.wire(AzureScheme::Http).as_str(),
            "http://127.0.0.1:10000/devstoreaccount1/general"
        );
        assert_eq!(
            emulator.canonical().as_str(),
            "az://127.0.0.1:10000/devstoreaccount1/general"
        );

        // No port written, none invented.
        let azure = AzureChannelUrl::parse("az://acct.blob.core.windows.net/general").unwrap();
        assert_eq!(azure.host().to_string(), "acct.blob.core.windows.net");
        assert_eq!(azure.host().port(), None);
    }

    /// Every normalization the URL host parser performs is a way for a written
    /// config key and a looked-up host to disagree, unless both go through the same
    /// parser. They do: this is that parser, and these are the classes it has to
    /// collapse.
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
            ("xn--n-nga1b.blob.example", "xn--n-nga1b.blob.example"),
            ("[0:0:0:0:0:0:0:1]:10000", "[::1]:10000"),
            ("[::1]:10000", "[::1]:10000"),
            ("0x7f.1", "127.0.0.1"),
            ("127.0.0.1", "127.0.0.1"),
            ("acct.blob.core.windows.net.", "acct.blob.core.windows.net"),
            ("acct.blob.core.windows.net", "acct.blob.core.windows.net"),
        ] {
            let host = AzureHost::parse(written)
                .unwrap_or_else(|err| panic!("{written} should parse: {err}"));
            assert_eq!(host.to_string(), canonical, "{written}");

            // Display and parse round-trip, so a key written out of an `AzureHost`
            // parses back to the same host…
            let reparsed = AzureHost::parse(canonical).unwrap();
            assert_eq!(reparsed, host, "{written}");
            // …and equal hosts hash equally, so they land on the same map entry.
            assert_eq!(hash_of(&host), hash_of(&reparsed), "{written}");
        }
    }

    /// A written port is part of the endpoint's identity: nothing here knows the
    /// scheme, so nothing here can call 443 or 80 redundant.
    #[test]
    fn host_equality_is_not_scheme_relative() {
        let with_port = AzureHost::parse("azurite.local:443").unwrap();
        let without = AzureHost::parse("azurite.local").unwrap();
        assert_ne!(with_port, without);
        assert_ne!(with_port, AzureHost::parse("azurite.local:80").unwrap());
        assert_eq!(with_port.to_string(), "azurite.local:443");
    }

    /// A config key is a bare authority; anything else is a mistake worth naming
    /// rather than silently reinterpreting.
    #[test]
    fn host_rejects_anything_that_is_not_a_bare_authority() {
        // A name DNS cannot resolve and a port nothing can connect to: `wire()`
        // would otherwise hand out `https://host:0/…`, and a bare `host:` would be
        // silently read as the portless host, a different endpoint entirely.
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

    fn hash_of(host: &AzureHost) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        host.hash(&mut hasher);
        hasher.finish()
    }

    /// The path-style recipe, asserted string by string, because every field but
    /// `container` differs from host-style and each one fails silently when it is
    /// wrong: a missing `account_name` becomes a 403, a trailing slash becomes
    /// `//container/…`, and a `root` that skips one segment too few writes the
    /// whole channel one directory too deep.
    #[cfg(feature = "opendal")]
    #[test]
    fn azblob_config_under_path_style() {
        let channel =
            AzureChannelUrl::parse("az://127.0.0.1:10000/devstoreaccount1/general/mychannel")
                .unwrap();
        let config = azblob_config(
            &AzureCredentials::AccountKey("key".into()),
            &channel,
            AzureEndpoint {
                scheme: AzureScheme::Http,
                addressing: Addressing::PathStyle,
            },
        )
        .unwrap();

        assert_eq!(
            config.endpoint.as_deref(),
            Some("http://127.0.0.1:10000/devstoreaccount1")
        );
        assert_eq!(config.account_name.as_deref(), Some("devstoreaccount1"));
        assert_eq!(config.container, "general");
        assert_eq!(config.root.as_deref(), Some("/mychannel"));
        assert_eq!(config.account_key.as_deref(), Some("key"));

        let endpoint = config.endpoint.unwrap();
        assert!(!endpoint.ends_with('/'), "{endpoint}");
        let root = config.root.unwrap();
        assert!(
            !root.contains("general"),
            "the container must not appear in the root: {root}"
        );
        assert!(
            !root.contains("devstoreaccount1"),
            "the account must not appear in the root: {root}"
        );
    }

    /// A channel that is a bare `account/container` leaves nothing for the root,
    /// which must still be `/` and not the empty string opendal would treat as a
    /// relative path.
    #[cfg(feature = "opendal")]
    #[test]
    fn azblob_config_path_style_without_a_prefix() {
        let channel =
            AzureChannelUrl::parse("az://127.0.0.1:10000/devstoreaccount1/general").unwrap();
        let config = azblob_config(
            &AzureCredentials::SasToken("?sv=token".into()),
            &channel,
            AzureEndpoint {
                scheme: AzureScheme::Http,
                addressing: Addressing::PathStyle,
            },
        )
        .unwrap();

        assert_eq!(config.root.as_deref(), Some("/"));
        assert_eq!(config.container, "general");
        // The leading `?` is stripped exactly once, wherever the token came from.
        assert_eq!(config.sas_token.as_deref(), Some("sv=token"));
    }

    /// Host-style is the shape every existing caller uses, so honouring
    /// path-style must not have moved it.
    #[cfg(feature = "opendal")]
    #[test]
    fn azblob_config_under_host_style_is_unchanged() {
        let channel =
            AzureChannelUrl::parse("az://stcondachannel.blob.core.windows.net/general/sub/dir")
                .unwrap();
        let config = azblob_config(
            &AzureCredentials::SasToken("sv=token".into()),
            &channel,
            AzureEndpoint::default(),
        )
        .unwrap();

        assert_eq!(
            config.endpoint.as_deref(),
            Some("https://stcondachannel.blob.core.windows.net")
        );
        assert_eq!(config.account_name.as_deref(), Some("stcondachannel"));
        assert_eq!(config.container, "general");
        assert_eq!(config.root.as_deref(), Some("/sub/dir"));
        assert_eq!(config.sas_token.as_deref(), Some("sv=token"));
        assert_eq!(config.account_key, None);
    }

    /// A prefix with a space arrives here percent-encoded and opendal encodes
    /// `root + path` again, so the root has to be handed over decoded.
    #[cfg(feature = "opendal")]
    #[test]
    fn azblob_config_decodes_the_root() {
        let channel =
            AzureChannelUrl::parse("az://acct.blob.core.windows.net/general/with%20space").unwrap();
        let config = azblob_config(
            &AzureCredentials::AccountKey("key".into()),
            &channel,
            AzureEndpoint::default(),
        )
        .unwrap();

        assert_eq!(config.root.as_deref(), Some("/with space"));
    }

    /// Account and container derivation is deliberately *not* part of parsing: the
    /// addressing style is config that does not exist yet when clap parses the
    /// argument, so an emulator URL must survive parsing and be rejected (or not)
    /// later, once its options entry is known.
    #[test]
    fn parse_defers_account_derivation() {
        let channel = channel("az://127.0.0.1:10000/devstoreaccount1/general");

        assert!(account_and_container(&channel, Addressing::HostStyle).is_err());
        assert!(account_and_container(&channel, Addressing::PathStyle).is_ok());
    }

    /// The rewrite is invisible in the URL a user reads back: under path-style it
    /// moves the *account* too, so a channel URL that says `devstoreaccount1` mints
    /// a SAS for whatever account the escaped `..` climbs out to.
    #[test]
    fn a_rewritten_path_is_rejected() {
        for (input, resolved) in [
            (
                "az://acct.blob.core.windows.net/general/%2e%2e/%2e%2e/othercontainer/x",
                "/othercontainer/x",
            ),
            (
                "az://127.0.0.1:10000/devstoreaccount1/general/%2e%2e/%2e%2e/otheraccount/othercontainer",
                "/otheraccount/othercontainer",
            ),
            (
                "az://acct.blob.core.windows.net/general/../../othercontainer",
                "/othercontainer",
            ),
            (
                "az://acct.blob.core.windows.net/general/./noarch",
                "/general/noarch",
            ),
        ] {
            let Err(err) = AzureChannelUrl::parse(input) else {
                panic!("expected a rejection for {input}");
            };
            assert!(
                matches!(&err, AzureUrlError::NonCanonicalPath { resolved: got, .. } if got == resolved),
                "unexpected error for {input}: {err}"
            );
            // Both spellings are in the message, so the user can see where the URL
            // they wrote would have gone.
            let message = err.to_string();
            assert!(message.contains(resolved), "{message}");
            assert!(message.contains("/general/"), "{message}");
        }
    }

    /// The paths that must keep parsing: nothing about rejecting rewrites may
    /// narrow what an ordinary channel URL can say.
    #[test]
    fn unrewritten_paths_still_parse() {
        for (input, path) in [
            (
                "az://acct.blob.core.windows.net/general/prefix",
                "/general/prefix",
            ),
            ("az://acct.blob.core.windows.net/general/", "/general/"),
            ("az://acct.blob.core.windows.net/", "/"),
            ("az://acct.blob.core.windows.net", "/"),
            (
                "az://acct.blob.core.windows.net/general/with%20space",
                "/general/with%20space",
            ),
            (
                "az://acct.blob.core.windows.net/general/p?sv=token#frag",
                "/general/p",
            ),
            // A dot inside a segment is not a dot segment.
            (
                "az://acct.blob.core.windows.net/general/..hidden/...",
                "/general/..hidden/...",
            ),
        ] {
            assert_eq!(channel(input).canonical().path(), path, "{input}");
        }
    }

    /// A segment that cannot become a blob name is refused, rather than becoming a
    /// different blob name than the URL says.
    #[test]
    fn segments_that_cannot_name_a_blob_are_rejected() {
        assert!(matches!(
            AzureChannelUrl::parse("az://acct.blob.core.windows.net/general/%ff"),
            Err(AzureUrlError::NonUtf8Path { .. })
        ));

        // Both spellings: `url` normalizes the hex digits' case but not the escape.
        for input in [
            "az://acct.blob.core.windows.net/general/a%2Fb",
            "az://acct.blob.core.windows.net/general/a%2fb",
        ] {
            assert!(
                matches!(
                    AzureChannelUrl::parse(input),
                    Err(AzureUrlError::EncodedSlashInPath(_))
                ),
                "{input}"
            );
        }

        // A percent escape that is valid UTF-8 is still a legitimate segment.
        assert_eq!(
            channel("az://acct.blob.core.windows.net/general/caf%C3%A9")
                .canonical()
                .path(),
            "/general/caf%C3%A9"
        );
    }

    /// `--https-only` restricts the SAS to TLS, so a host configured for http would
    /// get a token it cannot use. Nothing else about the grant may move with it.
    #[cfg(feature = "clap")]
    #[test]
    fn https_only_follows_the_configured_scheme() {
        let coordinates = coordinates("acct", "general");
        let args = |scheme| {
            generate_sas_args(
                &coordinates.account,
                &coordinates.container,
                "cw",
                "2030-01-01T00:00:00Z",
                scheme,
            )
        };

        assert!(args(AzureScheme::Https).contains(&"--https-only"));
        assert!(!args(AzureScheme::Http).contains(&"--https-only"));

        for scheme in [AzureScheme::Https, AzureScheme::Http] {
            let args = args(scheme);
            assert!(args.windows(2).any(|pair| pair == ["--permissions", "cw"]));
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--expiry", "2030-01-01T00:00:00Z"])
            );
            assert!(args.contains(&"--as-user"));
            assert!(args.windows(2).any(|pair| pair == ["--auth-mode", "login"]));
        }
    }
}

#[cfg(test)]
mod debug_redaction_tests {
    use super::*;

    #[test]
    fn debug_never_prints_secret() {
        for creds in [
            AzureCredentials::AccountKey("supersecretkey".into()),
            AzureCredentials::SasToken("sig=deadbeef".into()),
        ] {
            let out = format!("{creds:?}");
            assert!(out.contains("REDACTED"), "not redacted: {out}");
            assert!(!out.contains("supersecret"));
            assert!(!out.contains("deadbeef"));
        }
    }

    /// An inline SAS reaches the wire and nothing else. Every other spelling of the
    /// channel is a log line or an error message waiting to happen.
    #[test]
    fn only_the_wire_spelling_carries_the_signature() {
        let channel = AzureChannelUrl::parse(
            "az://acct.blob.core.windows.net/general/p?sv=2024-11-04&sig=SECRETSIG&se=z",
        )
        .unwrap();

        for shown in [
            channel.canonical().to_string(),
            channel.to_string(),
            format!("{channel:?}"),
        ] {
            assert!(!shown.contains("SECRETSIG"), "signature leaked: {shown}");
            // The rest of the grant is not secret and is worth showing.
            assert!(shown.contains("sv=2024-11-04"), "over-redacted: {shown}");
            assert!(shown.contains("se=z"), "over-redacted: {shown}");
        }

        assert!(
            channel
                .wire(AzureScheme::Https)
                .to_string()
                .contains("sig=SECRETSIG"),
            "the wire spelling must keep the signature that authenticates the request"
        );
    }
}
