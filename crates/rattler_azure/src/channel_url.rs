use std::borrow::Cow;

use url::Url;

use crate::{AzureHost, AzureScheme, AzureUrlError};

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AzureChannelUrl {
    host: AzureHost,

    path: EncodedPath,

    /// A SAS token may be written inline.
    query: Option<String>,

    fragment: Option<String>,
}

const PATH_SEPARATORS: [char; 2] = ['/', '\\'];

const QUERY_OR_FRAGMENT_MARKERS: [char; 2] = ['?', '#'];

const AUTHORITY_TERMINATORS: [char; 4] = ['/', '\\', '?', '#'];

impl AzureChannelUrl {
    /// Parse and validate an `az://` channel URL.
    pub fn parse(value: &str) -> Result<Self, AzureUrlError> {
        let rest = strip_az_scheme(value)
            .ok_or_else(|| AzureUrlError::InvalidScheme(value.to_string()))?;

        let (authority, tail) = split_authority(rest);
        let host = AzureHost::parse(authority)?;
        let url = parse_wire_url(value, authority, tail)?;

        let written_segments = decode_written_path_segments(tail)?;
        if let Some(segment) = written_segments.iter().find(|s| s.is_dot_segment()) {
            return Err(AzureUrlError::DotSegmentInPath(segment.raw.to_string()));
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
            path: EncodedPath(url.path().to_string()),
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
            text.push_str(&sas.spell(query));
        }
        if let Some(fragment) = &self.fragment {
            text.push('#');
            text.push_str(&sas.spell(fragment));
        }

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

    pub(crate) fn path(&self) -> &EncodedPath {
        &self.path
    }
}

/// A URL's path in wire form with a leading `/` and percent-encoded
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct EncodedPath(String);

impl EncodedPath {
    /// The still-encoded segments
    pub(crate) fn segments(&self) -> std::str::Split<'_, char> {
        self.0.strip_prefix('/').unwrap_or(&self.0).split('/')
    }

    /// The percent-decoded segments
    #[cfg(feature = "opendal")]
    pub(crate) fn decoded_segments(&self) -> impl Iterator<Item = Cow<'_, str>> {
        self.segments().map(|segment| {
            percent_encoding::percent_decode_str(segment)
                .decode_utf8()
                .expect("a parsed channel path decodes to UTF-8")
        })
    }
}

impl std::fmt::Display for EncodedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for AzureChannelUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.canonical())
    }
}

impl std::fmt::Debug for AzureChannelUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

impl Sas {
    fn spell<'a>(self, text: &'a str) -> Cow<'a, str> {
        match self {
            Self::Exposed => Cow::Borrowed(text),
            Self::Masked => Cow::Owned(mask_sas_signature(text)),
        }
    }
}

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

fn strip_az_scheme(value: &str) -> Option<&str> {
    let (scheme, rest) = value.split_once("://")?;
    scheme.eq_ignore_ascii_case("az").then_some(rest)
}

fn split_authority(rest: &str) -> (&str, &str) {
    let authority_end = rest.find(AUTHORITY_TERMINATORS).unwrap_or(rest.len());
    rest.split_at(authority_end)
}

/// Parses `<authority><tail>` as an `https` URL
fn parse_wire_url(value: &str, authority: &str, tail: &str) -> Result<Url, AzureUrlError> {
    Url::parse(&format!("https://{authority}{tail}")).map_err(|source| AzureUrlError::InvalidUrl {
        value: value.to_string(),
        source,
    })
}

/// One segment of a url path
struct WrittenSegment<'a> {
    raw: &'a str,
    decoded: String,
}

impl WrittenSegment<'_> {
    fn is_dot_segment(&self) -> bool {
        self.decoded == "." || self.decoded == ".."
    }
}

/// Percent-decodes and UTF-8-validates every segment of the url tail
fn decode_written_path_segments(tail: &str) -> Result<Vec<WrittenSegment<'_>>, AzureUrlError> {
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
            Ok(WrittenSegment {
                raw: segment,
                decoded: decoded.into_owned(),
            })
        })
        .collect()
}

/// Whether an empty segment appears anywhere but at the end of the path
fn has_empty_segments(segments: &[&str]) -> bool {
    let last = segments.len().saturating_sub(1);
    segments[..last].iter().any(|segment| segment.is_empty())
}

/// The first `%` that does not start a valid escape, and the malformed escape
/// itself
fn malformed_percent_escape_in(segments: &[&str]) -> Option<(String, String)> {
    segments.iter().find_map(|segment| {
        malformed_percent_escape(segment).map(|escape| (segment.to_string(), escape))
    })
}

/// The first `%` in `segment` that does not begin a valid escape, with up to
/// two following characters
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

    #[track_caller]
    fn assert_rejects(inputs: &[&str], expected: fn(&AzureUrlError) -> bool) {
        for input in inputs {
            match AzureChannelUrl::parse(input) {
                Ok(_) => panic!("expected a rejection for {input}"),
                Err(err) => assert!(expected(&err), "wrong error for {input}: {err:?}"),
            }
        }
    }

    #[track_caller]
    fn assert_canonical_paths(cases: &[(&str, &str)]) {
        for (input, path) in cases {
            assert_eq!(channel(input).canonical().path(), *path, "{input}");
        }
    }

    #[test]
    fn parse_requires_the_az_scheme() {
        assert_rejects(
            &[
                "https://acct.blob.core.windows.net/general",
                "http://acct.blob.core.windows.net/general",
                "ftp://acct.blob.core.windows.net/general",
                "acct.blob.core.windows.net/general",
            ],
            |err| matches!(err, AzureUrlError::InvalidScheme(_)),
        );
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

        // a host on 443 is not the same endpoint as the same host with
        // no port, because the scheme that would make them equal is not known here.
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
        assert_rejects(
            &[
                "az://acct.blob.core.windows.net/general/%2e%2e/%2e%2e/othercontainer/x",
                "az://127.0.0.1:10000/devstoreaccount1/general/%2e%2e/%2e%2e/otheraccount/othercontainer",
                "az://acct.blob.core.windows.net/general/../../othercontainer",
                "az://acct.blob.core.windows.net/general/./noarch",
                "az://acct.blob.core.windows.net/general/%2E%2E/othercontainer",
            ],
            |err| matches!(err, AzureUrlError::DotSegmentInPath(_)),
        );
    }

    #[test]
    fn an_empty_segment_is_rejected() {
        assert_rejects(
            &[
                "az://acct.blob.core.windows.net//general/noarch",
                "az://acct.blob.core.windows.net/general//noarch",
                "az://127.0.0.1:10000//devstoreaccount1/general",
            ],
            |err| matches!(err, AzureUrlError::EmptyPathSegment { .. }),
        );

        assert_canonical_paths(&[("az://acct.blob.core.windows.net/general/", "/general/")]);
    }

    #[test]
    fn a_malformed_percent_escape_is_rejected() {
        assert_rejects(
            &[
                "az://acct.blob.core.windows.net/general/gen%eral",
                "az://acct.blob.core.windows.net/general/100%",
                "az://acct.blob.core.windows.net/general/%zz",
            ],
            |err| matches!(err, AzureUrlError::MalformedPercentEscape { .. }),
        );
    }

    #[test]
    fn unrewritten_paths_still_parse() {
        assert_canonical_paths(&[
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
        ]);
    }

    #[test]
    fn segments_that_cannot_name_a_blob_are_rejected() {
        assert_rejects(
            &[
                "az://acct.blob.core.windows.net/general/%ff",
                // only becomes invalid UTF-8 once the parser encodes the raw character
                "az://acct.blob.core.windows.net/general/caf%C3é",
                "az://acct.blob.core.windows.net/general/%C3é",
            ],
            |err| matches!(err, AzureUrlError::NonUtf8Path { .. }),
        );

        // An encoded slash past the container is a blob name containing a slash,
        // which Azure supports
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

        assert_canonical_paths(&[
            (
                "az://acct.blob.core.windows.net/general/café",
                "/general/caf%C3%A9",
            ),
            (
                "az://acct.blob.core.windows.net/general/with space",
                "/general/with%20space",
            ),
            (
                "az://acct.blob.core.windows.net/general/caf%C3%A9",
                "/general/caf%C3%A9",
            ),
        ]);
    }
}

#[cfg(test)]
mod debug_redaction_tests {
    use super::*;
    use crate::test_support::channel;

    fn masked_spellings(channel: &AzureChannelUrl) -> [String; 3] {
        [
            channel.canonical().to_string(),
            channel.to_string(),
            format!("{channel:?}"),
        ]
    }

    #[test]
    fn only_the_wire_spelling_carries_the_signature() {
        let signed =
            channel("az://acct.blob.core.windows.net/general/p?sv=2024-11-04&sig=SECRETSIG&se=z");

        for shown in masked_spellings(&signed) {
            assert!(!shown.contains("SECRETSIG"), "signature leaked: {shown}");
            assert!(shown.contains("sv=2024-11-04"), "over-redacted: {shown}");
            assert!(shown.contains("se=z"), "over-redacted: {shown}");
        }

        let fragmented = channel("az://acct.blob.core.windows.net/general/p?sv=1#sig=SECRETFRAG");
        for shown in masked_spellings(&fragmented) {
            assert!(!shown.contains("SECRETFRAG"), "signature leaked: {shown}");
        }

        assert!(
            signed
                .wire(AzureScheme::Https)
                .to_string()
                .contains("sig=SECRETSIG"),
            "the wire spelling must keep the signature that authenticates the request"
        );
    }
}
