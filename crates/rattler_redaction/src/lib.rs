use std::borrow::Cow;
use std::ops::Range;

use url::Url;

/// A default string to use for redaction.
pub const DEFAULT_REDACTION_STR: &str = "********";

/// The signature is the credential itself; the rest of such a URL is inert without
/// it and stays readable.
const SIGNATURE_PARAM: &str = "sig";

/// The byte range of a `sig` parameter's value, starting right after the
/// `=` at `equals` (an offset into `pair = &text[separator + 1..]`) and
/// ending at the first character that cannot be part of the value.
///
/// Returns `None` for an empty value, since there is nothing to redact.
fn signature_value_span(text: &str, separator: usize, equals: usize) -> Option<Range<usize>> {
    let value = separator + 1 + equals + 1;
    let end = value
        + text[value..]
            .find(|c: char| c.is_whitespace() || "&\"',)]}".contains(c))
            .unwrap_or(text.len() - value);
    if end == value {
        return None;
    }
    Some(value..end)
}

/// Mask the signature of a pre-signed URL wherever one appears in `text`.
///
/// Takes text rather than a [`Url`] because that is the shape the leak has:
/// storage backends quote the request URL inside error messages.
pub fn redact_signatures_in_text<'a>(text: &'a str, redaction: &str) -> Cow<'a, str> {
    let mut out = String::new();
    let mut written = 0;

    for (separator, _) in text.char_indices().filter(|(_, c)| *c == '?' || *c == '&') {
        let pair = &text[separator + 1..];
        let Some(equals) = pair.find('=') else {
            continue;
        };
        if !pair[..equals].eq_ignore_ascii_case(SIGNATURE_PARAM) {
            continue;
        }

        let Some(Range { start: value, end }) = signature_value_span(text, separator, equals)
        else {
            continue;
        };
        if value < written {
            continue;
        }

        out.push_str(&text[written..value]);
        out.push_str(redaction);
        written = end;
    }

    if written == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[written..]);
    Cow::Owned(out)
}

/// Masks the known secret patterns in a URL for display in logs and error
/// messages: the password in the userinfo (the username stays), the token in
/// a `/t/<token>/` path, and a `sig` query parameter such as an Azure SAS
/// signature. Other query parameters and the fragment are left untouched, so
/// the URL stays recognizable for debugging.
///
/// Use [`strip_url_for_serialization`] instead when the URL is written into
/// durable output.
///
/// The `redaction` argument replaces each masked secret. For consistency
/// between applications it is recommended to pass [`DEFAULT_REDACTION_STR`].
///
/// # Example
///
/// ```rust
/// # use rattler_redaction::{redact_url_for_display, Redact, DEFAULT_REDACTION_STR};
/// # use url::Url;
///
/// let url = Url::parse("https://conda.anaconda.org/t/12345677/conda-forge/noarch/repodata.json").unwrap();
/// let redacted_url = redact_url_for_display(&url, DEFAULT_REDACTION_STR).unwrap_or(url.clone());
/// // or you can use the shorthand
/// let redacted_url = url.redact();
/// ```
pub fn redact_url_for_display(url: &Url, redaction: &str) -> Option<Url> {
    let mut url = url.clone();
    if url.password().is_some() {
        url.set_password(Some(redaction)).ok()?;
    }

    if let Some(query) = url.query()
        && let Cow::Owned(masked) = redact_signatures_in_text(&format!("?{query}"), redaction)
    {
        url.set_query(Some(&masked[1..]));
    }

    // A URL that cannot be a base has no `/t/<token>/` to mask but may still
    // have had a signature masked above; returning `None` here would throw
    // that masking away, since callers fall back to the unredacted URL.
    let token_prefixed = url.path_segments().is_some_and(|mut segments| {
        matches!((segments.next(), segments.next()), (Some("t"), Some(_)))
    });

    if token_prefixed {
        let remainder = url
            .path_segments()
            .into_iter()
            .flatten()
            .skip(2)
            .collect::<Vec<_>>();
        let mut redacted_path = format!(
            "t/{redaction}{separator}",
            separator = if remainder.is_empty() { "" } else { "/" },
        );

        for (idx, segment) in remainder.iter().enumerate() {
            redacted_path.push_str(segment);
            // if the original url ends with a slash, we need to add it to the redacted path
            if idx < remainder.len() - 1 {
                redacted_path.push('/');
            }
        }

        url.set_path(&redacted_path);
    }

    Some(url)
}

/// Deprecated name of [`redact_url_for_display`].
#[deprecated(since = "0.2.3", note = "renamed to `redact_url_for_display`")]
pub fn redact_known_secrets_from_url(url: &Url, redaction: &str) -> Option<Url> {
    redact_url_for_display(url, redaction)
}

/// Strips a URL down to what is safe to write into durable output such as
/// canonical match specs, repodata, or lockfiles: the userinfo, a
/// `/t/<token>/` path prefix, and the query string are dropped, as is any
/// fragment that is not a conda artifact digest (`md5:<hex>` or
/// `sha256:<hex>`), which is a content address rather than a secret.
///
/// Secrets are removed rather than masked so that what remains is a URL that
/// still resolves: the same channel, reached without the credentials. Query
/// strings go wholesale instead of by key, because arbitrary services use
/// arbitrary parameter names for credentials and no allowlist can promise
/// otherwise. Stripping an already stripped URL leaves it unchanged.
///
/// Use [`redact_url_for_display`] instead when the URL is only shown to a
/// human and should stay recognizable.
pub fn strip_url_for_serialization(url: &Url) -> Url {
    fn is_artifact_digest(fragment: &str) -> bool {
        let Some((algorithm, digest)) = fragment.split_once(':') else {
            return false;
        };
        matches!(algorithm, "md5" | "sha256")
            && !digest.is_empty()
            && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    let mut url = url.clone();
    if !url.username().is_empty() || url.password().is_some() {
        let _ = url.set_username("");
        let _ = url.set_password(None);
    }

    // A `/t/<token>/` prefix authenticates the request; the same location is
    // reachable without it. An empty token means the path is just `/t/`,
    // which is a channel named `t` rather than a credential.
    let path_without_token = url.path_segments().and_then(|mut segments| {
        match (segments.next(), segments.next()) {
            // The remaining segments include the empty one a trailing slash
            // produces, so rejoining them preserves it.
            (Some("t"), Some(token)) if !token.is_empty() => {
                Some(format!("/{}", segments.collect::<Vec<_>>().join("/")))
            }
            _ => None,
        }
    });
    if let Some(path) = path_without_token {
        url.set_path(&path);
    }

    url.set_query(None);
    if url
        .fragment()
        .is_some_and(|fragment| !is_artifact_digest(fragment))
    {
        url.set_fragment(None);
    }

    url
}

/// A trait to redact known secrets from a type.
pub trait Redact {
    /// Redacts any secrets from this instance.
    fn redact(self) -> Self;
}

#[cfg(feature = "reqwest-middleware")]
impl Redact for reqwest_middleware::Error {
    fn redact(self) -> Self {
        if let Some(url) = self.url() {
            let redacted_url =
                redact_url_for_display(url, DEFAULT_REDACTION_STR).unwrap_or_else(|| url.clone());
            self.with_url(redacted_url)
        } else {
            self
        }
    }
}

#[cfg(feature = "reqwest")]
impl Redact for reqwest::Error {
    fn redact(self) -> Self {
        if let Some(url) = self.url() {
            let redacted_url =
                redact_url_for_display(url, DEFAULT_REDACTION_STR).unwrap_or_else(|| url.clone());
            self.with_url(redacted_url)
        } else {
            self
        }
    }
}

impl Redact for Url {
    fn redact(self) -> Self {
        redact_url_for_display(&self, DEFAULT_REDACTION_STR).unwrap_or(self)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_redact_signatures_in_text() {
        let message = "unexpected status code 403, url=https://acct.blob.core.windows.net/c/p?sv=2025-01-05&se=2026-08-05T00%3A00Z&sig=aBcD%2Fefg%3D, op=stat";
        assert_eq!(
            redact_signatures_in_text(message, DEFAULT_REDACTION_STR),
            format!(
                "unexpected status code 403, url=https://acct.blob.core.windows.net/c/p?sv=2025-01-05&se=2026-08-05T00%3A00Z&sig={DEFAULT_REDACTION_STR}, op=stat"
            )
        );

        assert!(matches!(
            redact_signatures_in_text("https://prefix.dev/conda-forge?a=b", "X"),
            Cow::Borrowed(_)
        ));

        assert_eq!(
            redact_signatures_in_text("https://h/p?design=keep&sig=drop", "X"),
            "https://h/p?design=keep&sig=X"
        );

        assert_eq!(
            Url::from_str("https://acct.blob.core.windows.net/c/p?sv=2025-01-05&sig=secret")
                .unwrap()
                .redact()
                .to_string(),
            format!(
                "https://acct.blob.core.windows.net/c/p?sv=2025-01-05&sig={DEFAULT_REDACTION_STR}"
            )
        );
    }

    #[test]
    fn a_url_that_cannot_be_a_base_still_has_its_signature_masked() {
        let redacted = redact_url_for_display(
            &Url::from_str("az:container/repodata.json?sig=secret").unwrap(),
            DEFAULT_REDACTION_STR,
        )
        .expect("a signature must be masked even with no path segments to inspect");

        assert!(!redacted.to_string().contains("secret"), "{redacted}");
    }

    #[test]
    fn test_redact_url_for_display() {
        assert_eq!(
            redact_url_for_display(
                &Url::from_str(
                    "https://conda.anaconda.org/t/12345677/conda-forge/noarch/repodata.json"
                )
                .unwrap(),
                DEFAULT_REDACTION_STR
            ),
            Some(
                Url::from_str(
                    &format!("https://conda.anaconda.org/t/{DEFAULT_REDACTION_STR}/conda-forge/noarch/repodata.json")
                )
                .unwrap()
            )
        );

        // should stay as is
        assert_eq!(
            redact_url_for_display(
                &Url::from_str("https://conda.anaconda.org/conda-forge/noarch/repodata.json")
                    .unwrap(),
                "helloworld"
            )
            .unwrap(),
            Url::from_str("https://conda.anaconda.org/conda-forge/noarch/repodata.json").unwrap(),
        );

        let redacted = redact_url_for_display(
            &Url::from_str("https://user:secret@prefix.dev/conda-forge").unwrap(),
            DEFAULT_REDACTION_STR,
        )
        .unwrap();

        assert_eq!(
            redacted.to_string(),
            format!("https://user:{DEFAULT_REDACTION_STR}@prefix.dev/conda-forge")
        );

        let redacted = redact_url_for_display(
            &Url::from_str("https://user:secret@prefix.dev/conda-forge/").unwrap(),
            DEFAULT_REDACTION_STR,
        )
        .unwrap();

        assert_eq!(
            redacted.to_string(),
            format!("https://user:{DEFAULT_REDACTION_STR}@prefix.dev/conda-forge/")
        );
    }

    #[test]
    fn test_strip_url_for_serialization() {
        let url = Url::parse(
            "https://user:password@prefix.dev/t/path-token/channel?auth=session&keep=value#ticket=fragment-token",
        )
        .unwrap();
        let stripped = strip_url_for_serialization(&url);

        assert_eq!(stripped.as_str(), "https://prefix.dev/channel");
        // Stripping is a fixed point, so repeated renders agree.
        assert_eq!(strip_url_for_serialization(&stripped), stripped);

        // A trailing slash marks a directory, so it has to survive.
        assert_eq!(
            strip_url_for_serialization(
                &Url::parse("https://prefix.dev/t/path-token/channel/").unwrap()
            )
            .as_str(),
            "https://prefix.dev/channel/"
        );

        // `/t/` with nothing after it is a channel named `t`, not a token.
        let named_t = Url::parse("https://conda.anaconda.org/t/").unwrap();
        assert_eq!(strip_url_for_serialization(&named_t), named_t);

        let digest_url = Url::parse("https://prefix.dev/pkg.conda#sha256:deadbeef").unwrap();
        assert_eq!(strip_url_for_serialization(&digest_url), digest_url);
    }
}
