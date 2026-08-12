use url::Url;

/// A default string to use for redaction.
pub const DEFAULT_REDACTION_STR: &str = "********";

/// Anaconda channels are not always publicly available. This function checks if a URL contains a
/// secret by identifying whether it contains certain patterns. If it does, the function returns a
/// modified URL where any secret has been masked.
///
/// The `redaction` argument can be used to specify a custom string that should be used to replace
/// a secret. For consistency between application it is recommended to pass
/// [`DEFAULT_REDACTION_STR`].
///
/// # Example
///
/// ```rust
/// # use rattler_redaction::{redact_known_secrets_from_url, Redact, DEFAULT_REDACTION_STR};
/// # use url::Url;
///
/// let url = Url::parse("https://conda.anaconda.org/t/12345677/conda-forge/noarch/repodata.json").unwrap();
/// let redacted_url = redact_known_secrets_from_url(&url, DEFAULT_REDACTION_STR).unwrap_or(url.clone());
/// // or you can use the shorthand
/// let redacted_url = url.redact();
/// ```
pub fn redact_known_secrets_from_url(url: &Url, redaction: &str) -> Option<Url> {
    let mut url = url.clone();
    if url.password().is_some() {
        url.set_password(Some(redaction)).ok()?;
    }

    let mut segments = url.path_segments()?;
    match (segments.next(), segments.next()) {
        (Some("t"), Some(_)) => {
            let remainder = segments.collect::<Vec<_>>();
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
            Some(url)
        }
        _ => Some(url),
    }
}

/// Returns a URL safe to serialize by removing userinfo, redacting known path
/// tokens, and treating all query and non-digest fragment contents as secret.
///
/// Query strings are intentionally replaced wholesale: arbitrary services can
/// use arbitrary parameter names for credentials, so a key allowlist or denylist
/// cannot provide a meaningful safety guarantee. Conda artifact fragments of the
/// form `md5:<hex>` or `sha256:<hex>` are retained.
pub fn redact_credentials_from_url(url: &Url) -> Url {
    fn is_artifact_digest(fragment: &str) -> bool {
        let Some((algorithm, digest)) = fragment.split_once(':') else {
            return false;
        };
        matches!(algorithm, "md5" | "sha256")
            && !digest.is_empty()
            && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    let mut url =
        redact_known_secrets_from_url(url, DEFAULT_REDACTION_STR).unwrap_or_else(|| url.clone());
    if !url.username().is_empty() || url.password().is_some() {
        let _ = url.set_username("");
        let _ = url.set_password(None);
    }
    if url.query().is_some() {
        url.set_query(Some(DEFAULT_REDACTION_STR));
    }
    if url
        .fragment()
        .is_some_and(|fragment| !is_artifact_digest(fragment))
    {
        url.set_fragment(Some(DEFAULT_REDACTION_STR));
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
            let redacted_url = redact_known_secrets_from_url(url, DEFAULT_REDACTION_STR)
                .unwrap_or_else(|| url.clone());
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
            let redacted_url = redact_known_secrets_from_url(url, DEFAULT_REDACTION_STR)
                .unwrap_or_else(|| url.clone());
            self.with_url(redacted_url)
        } else {
            self
        }
    }
}

impl Redact for Url {
    fn redact(self) -> Self {
        redact_known_secrets_from_url(&self, DEFAULT_REDACTION_STR).unwrap_or(self)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_remove_known_secrets_from_url() {
        assert_eq!(
            redact_known_secrets_from_url(
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
            redact_known_secrets_from_url(
                &Url::from_str("https://conda.anaconda.org/conda-forge/noarch/repodata.json")
                    .unwrap(),
                "helloworld"
            )
            .unwrap(),
            Url::from_str("https://conda.anaconda.org/conda-forge/noarch/repodata.json").unwrap(),
        );

        let redacted = redact_known_secrets_from_url(
            &Url::from_str("https://user:secret@prefix.dev/conda-forge").unwrap(),
            DEFAULT_REDACTION_STR,
        )
        .unwrap();

        assert_eq!(
            redacted.to_string(),
            format!("https://user:{DEFAULT_REDACTION_STR}@prefix.dev/conda-forge")
        );

        let redacted = redact_known_secrets_from_url(
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
    fn test_redact_credentials_from_url() {
        let url = Url::parse(
            "https://user:password@prefix.dev/t/path-token/channel?auth=session&keep=value#ticket=fragment-token",
        )
        .unwrap();

        assert_eq!(
            redact_credentials_from_url(&url).as_str(),
            "https://prefix.dev/t/********/channel?********#********"
        );

        let digest_url = Url::parse("https://prefix.dev/pkg.conda#sha256:deadbeef").unwrap();
        assert_eq!(redact_credentials_from_url(&digest_url), digest_url);
    }
}
