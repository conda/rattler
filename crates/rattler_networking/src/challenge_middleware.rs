//! Host-scoped middleware that reacts to `WWW-Authenticate` challenges by
//! acquiring a bearer token from a pluggable `AuthFlow` and replaying the
//! request once.
//!
//! The first `AuthFlow` implementation will be
//! `crate::trusted_publishing::TrustedPublishingFlow` (added in a later task).

use std::{
    collections::HashMap,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use serde::Deserialize;

/// One parsed challenge from a `WWW-Authenticate` response header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    /// The authentication scheme, e.g. `Bearer` (case preserved as sent).
    pub scheme: String,
    /// Auth parameters with lowercased keys, e.g. `realm` -> `prefix.dev`.
    /// `token68` payloads (e.g. base64 blobs after the scheme) are skipped.
    pub params: HashMap<String, String>,
}

/// Parse all challenges from every `WWW-Authenticate` header in `headers`.
///
/// Tolerant by design: malformed input yields fewer (or no) challenges,
/// never an error or panic. Handles multiple comma-separated challenges in
/// one header value as well as the header appearing multiple times.
pub fn parse_challenges(headers: &http::HeaderMap) -> Vec<Challenge> {
    headers
        .get_all(http::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(parse_header_value)
        .collect()
}

/// An auth scheme is a token of ASCII alphanumerics plus a few safe symbols.
/// Stricter than RFC 7235's `token` on purpose: it rejects line noise that
/// would otherwise be misread as a scheme.
fn is_valid_scheme(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn parse_header_value(value: &str) -> Vec<Challenge> {
    let mut challenges: Vec<Challenge> = Vec::new();
    for item in split_commas_respecting_quotes(value) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        // A new challenge starts with a scheme token; a continuation item is
        // a bare `key=value` auth-param belonging to the current challenge.
        let (first, rest) = match item.split_once(char::is_whitespace) {
            Some((first, rest)) => (first, Some(rest.trim())),
            None => (item, None),
        };
        if !first.contains('=') {
            if !is_valid_scheme(first) {
                continue;
            }
            challenges.push(Challenge {
                scheme: first.to_string(),
                params: HashMap::new(),
            });
            if let (Some(rest), Some(challenge)) = (rest, challenges.last_mut())
                && let Some((key, val)) = parse_param(rest)
            {
                challenge.params.insert(key, val);
            }
        } else if let Some(challenge) = challenges.last_mut()
            && let Some((key, val)) = parse_param(item)
        {
            challenge.params.insert(key, val);
        }
    }
    challenges
}

/// Parse one `key=value` or `key="quoted value"` auth-param. Returns `None`
/// for non-params (e.g. token68 blobs like `YII=`, which have an empty
/// "value" after the trailing `=`).
fn parse_param(s: &str) -> Option<(String, String)> {
    let (key, value) = s.split_once('=')?;
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim();
    if key.is_empty() || value.is_empty() || !is_valid_scheme(&key) {
        return None;
    }
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    Some((key, value.replace("\\\"", "\"")))
}

/// Split on commas that are not inside a double-quoted string.
fn split_commas_respecting_quotes(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match c {
            '\\' if in_quotes && !escaped => escaped = true,
            '"' if !escaped => {
                in_quotes = !in_quotes;
                escaped = false;
            }
            ',' if !in_quotes => {
                parts.push(&s[start..i]);
                start = i + 1;
                escaped = false;
            }
            _ => escaped = false,
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Refresh tokens this long before their `exp` so a token does not become
/// invalid while a request is in flight.
#[allow(dead_code)]
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// A short-lived bearer token acquired by an auth flow (trait added in a later task).
///
/// `Deserialize`-transparent (a raw JSON string body deserializes directly
/// into it) and `Clone` so it can be shared between the cache and requests.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct BearerToken(String);

impl BearerToken {
    /// Wrap an existing token string.
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// The raw bearer token. Treat as sensitive; don't log it.
    pub fn secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BearerToken").field(&"<redacted>").finish()
    }
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct JwtClaims {
    exp: Option<u64>,
}

/// Best-effort extraction of the `exp` claim from a JWT-shaped token.
/// Returns `None` for opaque tokens, which are then cached without expiry.
#[allow(dead_code)]
fn jwt_expiration(token: &str) -> Option<SystemTime> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _signature = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    let claims: JwtClaims = serde_json::from_slice(&payload).ok()?;
    claims
        .exp
        .and_then(|exp| UNIX_EPOCH.checked_add(Duration::from_secs(exp)))
}

#[derive(Debug)]
struct CachedToken {
    #[allow(dead_code)]
    token: BearerToken,
    #[allow(dead_code)]
    expires_at: Option<SystemTime>,
}

impl CachedToken {
    #[allow(dead_code)]
    fn new(token: BearerToken) -> Self {
        let expires_at = jwt_expiration(token.secret());
        Self { token, expires_at }
    }

    #[allow(dead_code)]
    fn is_fresh(&self, now: SystemTime) -> bool {
        self.expires_at
            .is_none_or(|expires_at| now + TOKEN_REFRESH_MARGIN < expires_at)
    }
}

/// Cache state shared by all clones of one middleware instance.
#[derive(Debug, Default)]
#[allow(dead_code)]
enum TokenCache {
    /// No acquisition attempted yet.
    #[default]
    Empty,
    /// The flow reported "not applicable" or failed — stop asking.
    Disabled,
    /// A previously acquired token.
    Token(CachedToken),
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn header_map(values: &[&str]) -> http::HeaderMap {
        let mut headers = http::HeaderMap::new();
        for v in values {
            headers.append(
                http::header::WWW_AUTHENTICATE,
                http::HeaderValue::from_str(v).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn parses_single_bearer_challenge() {
        let challenges = parse_challenges(&header_map(&[r#"Bearer realm="prefix.dev""#]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].scheme, "Bearer");
        assert_eq!(challenges[0].params["realm"], "prefix.dev");
    }

    #[test]
    fn parses_multiple_challenges_in_one_header() {
        let challenges = parse_challenges(&header_map(&[
            r#"Bearer realm="prefix.dev", error="invalid_token", Basic realm="other""#,
        ]));
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].scheme, "Bearer");
        assert_eq!(challenges[0].params["realm"], "prefix.dev");
        assert_eq!(challenges[0].params["error"], "invalid_token");
        assert_eq!(challenges[1].scheme, "Basic");
        assert_eq!(challenges[1].params["realm"], "other");
    }

    #[test]
    fn parses_multiple_headers() {
        let challenges =
            parse_challenges(&header_map(&[r#"Bearer realm="a""#, r#"Basic realm="b""#]));
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].scheme, "Bearer");
        assert_eq!(challenges[1].scheme, "Basic");
    }

    #[test]
    fn quoted_commas_do_not_split_challenges() {
        let challenges = parse_challenges(&header_map(&[r#"Bearer realm="a,b""#]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].params["realm"], "a,b");
    }

    #[test]
    fn unquoted_params_and_case_insensitive_keys() {
        let challenges = parse_challenges(&header_map(&["Bearer REALM=prefix.dev"]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].params["realm"], "prefix.dev");
    }

    #[test]
    fn token68_payload_is_skipped_not_a_param() {
        // e.g. `Negotiate YII=` — the trailing blob is not a key=value param
        let challenges = parse_challenges(&header_map(&["Negotiate YII="]));
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].scheme, "Negotiate");
        assert!(challenges[0].params.is_empty());
    }

    #[test]
    fn garbage_yields_no_challenges_and_no_panic() {
        assert!(parse_challenges(&header_map(&["= = ="])).is_empty());
        assert!(parse_challenges(&header_map(&[",,,"])).is_empty());
        assert!(parse_challenges(&header_map(&[""])).is_empty());
        assert!(parse_challenges(&header_map(&["%%% ###"])).is_empty());
        assert!(parse_challenges(&http::HeaderMap::new()).is_empty());
    }

    #[test]
    fn bearer_token_debug_is_redacted() {
        let token = BearerToken::new("supersecret".to_string());
        let formatted = format!("{token:?}");
        assert!(!formatted.contains("supersecret"));
        assert!(formatted.contains("redacted"));
    }

    fn unsigned_jwt_with_exp(exp: u64) -> String {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("{header}.{payload}.")
    }

    #[test]
    fn jwt_expiration_reads_exp_claim() {
        let token = unsigned_jwt_with_exp(1_700_000_000);
        assert_eq!(
            jwt_expiration(&token),
            UNIX_EPOCH.checked_add(Duration::from_secs(1_700_000_000))
        );
    }

    #[test]
    fn opaque_token_has_no_expiration() {
        assert_eq!(jwt_expiration("not-a-jwt"), None);
    }

    #[test]
    fn cached_jwt_is_stale_inside_refresh_margin() {
        let token = BearerToken::new(unsigned_jwt_with_exp(1_700_000_000));
        let cached = CachedToken::new(token);
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000 - 30);
        assert!(!cached.is_fresh(now));
        let earlier = UNIX_EPOCH + Duration::from_secs(1_700_000_000 - 3600);
        assert!(cached.is_fresh(earlier));
    }
}
