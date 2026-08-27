use url::Url;

use crate::{AzureHost, AzureScheme, AzureUrlError};

/// A validated Azure Blob **channel** URL, which has two spellings: `az://…` as
/// the user writes it and in configuration, and `http(s)://…` on the wire.
///
/// The parts are stored rather than a `Url`, because a `Url`'s port is
/// scheme-relative: storing `az://host:443/…` as `https` drops the port, and
/// [`wire`](Self::wire) would then hand out `http://host/…`, a different endpoint.
/// [`AzureHost`] holds host and port explicitly and normalizes both without a
/// scheme. Every spelling is built from those same parts, so no two spellings can
/// disagree.
///
/// The wire scheme is an argument to [`wire`](Self::wire) rather than a field
/// because it comes from the matched `azure-options` entry, while
/// [`parse`](Self::parse) runs as a clap `value_parser`, before any config file is
/// read. `rattler-index` takes it from that entry; `rattler_upload` passes the
/// default, because it reads no config file at all.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AzureChannelUrl {
    host: AzureHost,

    /// The path as the URL Standard normalizes it: always a leading `/`, still
    /// percent-encoded.
    path: String,

    /// A SAS token may be written inline.
    query: Option<String>,

    /// Kept so [`canonical`](Self::canonical) spells the channel back the way the
    /// user wrote it. It reaches no server: an HTTP request carries only the path
    /// and query.
    fragment: Option<String>,
}

/// `\` is a path separator as much as `/` is: the URL Standard's special-scheme
/// parser (what the `url` crate implements for `http`/`https`, which is what
/// [`AzureChannelUrl::parse`] parses the tail as) folds `\` into `/` while
/// parsing a special-scheme URL's path, so anything validated against this
/// type's own idea of "separator" has to agree.
const PATH_SEPARATORS: [char; 2] = ['/', '\\'];

/// Starts a query (`?`) or fragment (`#`), ending whatever came before it.
const QUERY_OR_FRAGMENT_MARKERS: [char; 2] = ['?', '#'];

/// [`PATH_SEPARATORS`] and [`QUERY_OR_FRAGMENT_MARKERS`] combined, written out
/// because `const` arrays cannot be concatenated: anything that is none of
/// these can still be part of an authority.
const AUTHORITY_TERMINATORS: [char; 4] = ['/', '\\', '?', '#'];

impl AzureChannelUrl {
    /// Parse and validate an `az://` channel URL.
    ///
    /// The only accepted spelling is `az://<host>/<…>`. A bare `http(s)://` URL is
    /// not accepted, so there is one canonical spelling for an Azure channel.
    ///
    /// Account and container derivation happens in [`locate`](crate::locate), not
    /// here: it depends on which [`AzureEndpointKey`](crate::AzureEndpointKey) the
    /// URL matches, which is config that does not exist yet at clap parse time.
    pub fn parse(value: &str) -> Result<Self, AzureUrlError> {
        let rest = strip_az_scheme(value)
            .ok_or_else(|| AzureUrlError::InvalidScheme(value.to_string()))?;

        let (authority, tail) = split_authority(rest);
        let host = AzureHost::parse(authority)?;
        let url = parse_wire_url(value, authority, tail)?;

        let written_segments = decode_written_path_segments(tail)?;
        if let Some(segment) = has_dot_segment(&written_segments) {
            return Err(AzureUrlError::DotSegmentInPath(segment.to_string()));
        }

        let segments = url
            .path_segments()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if has_empty_segments(&segments) {
            return Err(AzureUrlError::EmptyPathSegment {
                path: url.path().to_string(),
            });
        }
        if let Some((segment, escape)) = malformed_percent_escape_in(&segments) {
            return Err(AzureUrlError::MalformedPercentEscape { segment, escape });
        }

        Ok(Self {
            host,
            path: url.path().to_string(),
            query: url.query().map(str::to_string),
            fragment: url.fragment().map(str::to_string),
        })
    }

    pub fn canonical(&self) -> Url {
        self.spelled("az", Sas::Masked)
    }

    pub fn wire(&self, scheme: AzureScheme) -> Url {
        self.spelled(scheme.as_str(), Sas::Exposed)
    }

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
            // Masked on the same terms as the query: this spelling is the one that
            // reaches logs and error messages, and a `sig` is no less a signature
            // for having been written after a `#`.
            match sas {
                Sas::Exposed => text.push_str(fragment),
                Sas::Masked => text.push_str(&mask_sas_signature(fragment)),
            }
        }
        // Cannot fail: the authority re-serializes to the normalized form it was
        // parsed from, and the path, query and fragment are already-encoded output
        // of a `Url` parse. Every host shape `AzureHost` can hold (normalized
        // domain, IPv4 literal, bracketed IPv6) is valid both to the special-scheme
        // host parser and to the opaque-host parser `az://` gets.
        Url::parse(&text).expect("a normalized authority, path and query is a valid URL")
    }

    pub fn host(&self) -> &AzureHost {
        &self.host
    }

    pub(crate) fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    pub(crate) fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    /// The still-encoded path segments, exactly as [`Url::path_segments`] would
    /// yield them for the wire spelling.
    pub(crate) fn path_segments(&self) -> std::str::Split<'_, char> {
        self.path.strip_prefix('/').unwrap_or(&self.path).split('/')
    }
}

impl std::fmt::Display for AzureChannelUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sas {
    Exposed,
    Masked,
}

/// The other SAS parameters (`sv`, `se`, `sp`, …) only describe the grant; `sig`
/// is the secret that makes it usable.
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

// URL schemes are case-insensitive and `Url` lowercases them, so `AZ://…`
// reaches every downstream `scheme() == "az"` comparison as `az`.
fn strip_az_scheme(value: &str) -> Option<&str> {
    const PREFIX: &str = "az://";
    let prefix = value.get(..PREFIX.len())?;
    prefix
        // `str` has no case-insensitive `starts_with`
        .eq_ignore_ascii_case(PREFIX)
        .then(|| &value[PREFIX.len()..])
}

/// Splits `az://<host>/<…>`'s tail into the authority and everything after
/// it. The authority runs to the first [`AUTHORITY_TERMINATORS`] character,
/// so it ends at the same point the URL Standard's special-scheme parser
/// would end it.
fn split_authority(rest: &str) -> (&str, &str) {
    let authority_end = rest.find(AUTHORITY_TERMINATORS).unwrap_or(rest.len());
    rest.split_at(authority_end)
}

/// Parses `<authority><tail>` as an `https` URL: the URL Standard's
/// special-scheme parser is what normalizes the path, query and fragment, and
/// [`AzureChannelUrl::wire`] hands them straight to an `http(s)` URL, so they
/// have to be normalized that same way.
fn parse_wire_url(value: &str, authority: &str, tail: &str) -> Result<Url, AzureUrlError> {
    Url::parse(&format!("https://{authority}{tail}")).map_err(|source| AzureUrlError::InvalidUrl {
        value: value.to_string(),
        source,
    })
}

/// Percent-decodes and UTF-8-validates every segment of the path as the user
/// wrote it, pairing each with the raw (still-encoded) text it came from.
///
/// The segments are taken from the text the user wrote, not from a `Url`,
/// because by the time a `Url` exists the pre-resolution evidence dot-segment
/// detection needs is gone: the URL Standard's special-scheme parser resolves
/// dot segments while parsing.
///
/// This is also the only place a segment's percent-decoded bytes are checked
/// for validity as UTF-8: every segment `Url::path_segments` later yields for
/// this same tail carries the identical escapes (the special-scheme parser
/// only adds percent-encoding around characters that were not already
/// escaped, and decoding those trivially succeeds), so re-decoding them there
/// would just repeat this check.
fn decode_written_path_segments(tail: &str) -> Result<Vec<(&str, String)>, AzureUrlError> {
    let written = match tail
        .split(QUERY_OR_FRAGMENT_MARKERS)
        .next()
        .unwrap_or_default()
    {
        "" => "/",
        path => path,
    };
    written
        .trim_start_matches(PATH_SEPARATORS)
        .split(PATH_SEPARATORS)
        .map(|segment| {
            let decoded = percent_encoding::percent_decode_str(segment)
                .decode_utf8()
                .map_err(|source| AzureUrlError::NonUtf8Path {
                    segment: segment.to_string(),
                    source,
                })?;
            Ok((segment, decoded.into_owned()))
        })
        .collect()
}

/// A dot segment — `%2e%2e` as much as `..` — anywhere in the path lets a path
/// reading as one container (path-style: one *account*) address another:
/// `/general/a/../../evil/x` eats backwards into the container from a segment
/// that reads as harmless, and `/general\..\..\evil/x` climbs exactly as far
/// because the special-scheme parser treats `\` as a separator too. Returns
/// the raw (still-encoded) offending segment, since that is what the error
/// quotes back to the user.
fn has_dot_segment<'a>(segments: &[(&'a str, String)]) -> Option<&'a str> {
    segments
        .iter()
        .find(|(_, decoded)| decoded == "." || decoded == "..")
        .map(|(raw, _)| *raw)
}

/// Whether an empty segment appears anywhere but at the end of the path.
///
/// An empty segment is not a blob name and not a container name, but
/// `path_segments()` yields it, so without this `az://host//general` reads as
/// "no container" and downgrades a granted fetch to anonymous. A trailing one
/// is just a trailing slash, so it is excluded rather than reported.
fn has_empty_segments(segments: &[&str]) -> bool {
    let last = segments.len().saturating_sub(1);
    segments[..last].iter().any(|segment| segment.is_empty())
}

/// The first `%` that does not start a valid escape, and the malformed escape
/// itself: the one encoding defect the user cannot be left to own.
/// `percent_decode` passes it through literally, so the fetch path sends
/// `gen%eral` while opendal re-encodes the decoded form to `gen%25eral` and
/// indexes under a different blob.
fn malformed_percent_escape_in(segments: &[&str]) -> Option<(String, String)> {
    segments.iter().find_map(|segment| {
        malformed_percent_escape(segment).map(|escape| (segment.to_string(), escape))
    })
}

fn malformed_percent_escape(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    bytes.iter().enumerate().find_map(|(index, byte)| {
        if *byte != b'%' {
            return None;
        }
        let escape = bytes
            .get(index..index + 3)
            .filter(|escape| escape[1..].iter().all(u8::is_ascii_hexdigit));
        escape
            .is_none()
            .then(|| segment[index..].chars().take(3).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{channel, container, located};

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
        assert_eq!(
            channel,
            channel
                .canonical()
                .as_str()
                .parse::<AzureChannelUrl>()
                .unwrap()
        );
    }

    #[test]
    fn spellings_cannot_disagree() {
        for input in [
            "az://acct.blob.core.windows.net/general/noarch",
            "az://127.0.0.1:10000/devstoreaccount1/general",
            "az://acct.blob.core.windows.net/general/with%20space?sv=token",
            // An IPv6 literal is the host shape most likely to break the canonical
            // rebuild, since it has to survive being re-parsed as an opaque host.
            "az://[::1]:10000/devstoreaccount1/general",
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

        let azure = AzureChannelUrl::parse("az://acct.blob.core.windows.net/general").unwrap();
        assert_eq!(azure.host().to_string(), "acct.blob.core.windows.net");
        assert_eq!(azure.host().port(), None);
    }

    #[test]
    fn a_rewritten_path_is_rejected() {
        for input in [
            "az://acct.blob.core.windows.net/general/%2e%2e/%2e%2e/othercontainer/x",
            "az://127.0.0.1:10000/devstoreaccount1/general/%2e%2e/%2e%2e/otheraccount/othercontainer",
            "az://acct.blob.core.windows.net/general/../../othercontainer",
            "az://acct.blob.core.windows.net/general/./noarch",
            "az://acct.blob.core.windows.net/general/%2E%2E/othercontainer",
        ] {
            assert!(
                matches!(
                    AzureChannelUrl::parse(input),
                    Err(AzureUrlError::DotSegmentInPath(_))
                ),
                "expected a rejection for {input}"
            );
        }
    }

    #[test]
    fn an_empty_segment_is_rejected() {
        for input in [
            "az://acct.blob.core.windows.net//general/noarch",
            "az://acct.blob.core.windows.net/general//noarch",
            "az://127.0.0.1:10000//devstoreaccount1/general",
        ] {
            assert!(
                matches!(
                    AzureChannelUrl::parse(input),
                    Err(AzureUrlError::EmptyPathSegment { .. })
                ),
                "expected a rejection for {input}"
            );
        }

        assert_eq!(
            channel("az://acct.blob.core.windows.net/general/")
                .canonical()
                .path(),
            "/general/"
        );
    }

    #[test]
    fn a_malformed_percent_escape_is_rejected() {
        for input in [
            "az://acct.blob.core.windows.net/general/gen%eral",
            "az://acct.blob.core.windows.net/general/100%",
            "az://acct.blob.core.windows.net/general/%zz",
        ] {
            assert!(
                matches!(
                    AzureChannelUrl::parse(input),
                    Err(AzureUrlError::MalformedPercentEscape { .. })
                ),
                "expected a rejection for {input}"
            );
        }
    }

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

    #[test]
    fn segments_that_cannot_name_a_blob_are_rejected() {
        assert!(matches!(
            AzureChannelUrl::parse("az://acct.blob.core.windows.net/general/%ff"),
            Err(AzureUrlError::NonUtf8Path { .. })
        ));

        // An encoded slash past the container is a blob name containing a slash,
        // which Azure supports. It cannot move the container, which is read from a
        // separate raw segment, and `ContainerName`'s charset admits neither `/`
        // nor `%`, so the boundary is held by the type rather than by banning the
        // escape everywhere.
        for input in [
            "az://acct.blob.core.windows.net/general/a%2Fb",
            "az://acct.blob.core.windows.net/general/a%2fb",
        ] {
            assert_eq!(
                located(input, &[]).container(),
                Some(&container("general")),
                "{input}"
            );
        }

        for (input, path) in [
            (
                "az://acct.blob.core.windows.net/general/café",
                "/general/caf%C3%A9",
            ),
            (
                "az://acct.blob.core.windows.net/general/with space",
                "/general/with%20space",
            ),
        ] {
            assert_eq!(channel(input).canonical().path(), path, "{input}");
        }

        assert_eq!(
            channel("az://acct.blob.core.windows.net/general/caf%C3%A9")
                .canonical()
                .path(),
            "/general/caf%C3%A9"
        );
    }
}

#[cfg(test)]
mod debug_redaction_tests {
    use super::*;

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
            assert!(shown.contains("sv=2024-11-04"), "over-redacted: {shown}");
            assert!(shown.contains("se=z"), "over-redacted: {shown}");
        }

        let fragmented =
            AzureChannelUrl::parse("az://acct.blob.core.windows.net/general/p?sv=1#sig=SECRETFRAG")
                .unwrap();
        for shown in [
            fragmented.canonical().to_string(),
            fragmented.to_string(),
            format!("{fragmented:?}"),
        ] {
            assert!(!shown.contains("SECRETFRAG"), "signature leaked: {shown}");
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
