use std::borrow::Cow;
use url::Url;

/// Parses the schema part of the human-readable channel. Returns the scheme part if it exists.
pub(crate) fn parse_scheme(channel: &str) -> Option<&str> {
    let scheme_end = channel.find("://")?;

    // Scheme part is too long
    if scheme_end > 11 {
        return None;
    }

    let scheme_part = &channel[0..scheme_end];
    let mut scheme_chars = scheme_part.chars();

    // First character must be alphabetic
    if scheme_chars.next().map(char::is_alphabetic) != Some(true) {
        return None;
    }

    // The rest must be alpha-numeric
    if scheme_chars.all(char::is_alphanumeric) {
        Some(scheme_part)
    } else {
        None
    }
}

pub(crate) fn add_trailing_slash(url: &Url) -> Cow<'_, Url> {
    let path = url.path();
    if path.ends_with('/') {
        Cow::Borrowed(url)
    } else {
        let mut url = url.clone();
        url.set_path(&format!("{path}/"));
        Cow::Owned(url)
    }
}
