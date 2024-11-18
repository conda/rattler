use std::{
    fmt::{Display, Formatter},
    ops::Deref,
    str::FromStr,
};

use rattler_redaction::Redact;
use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

/// A URL that always has a trailing slash. A trailing slash in a URL has
/// significance but users often forget to add it. This type is used to
/// normalize the use of the URL.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UrlWithTrailingSlash(Url);

impl Deref for UrlWithTrailingSlash {
    type Target = Url;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Url> for UrlWithTrailingSlash {
    fn as_ref(&self) -> &Url {
        &self.0
    }
}

impl From<Url> for UrlWithTrailingSlash {
    fn from(url: Url) -> Self {
        let path = url.path();
        if path.ends_with('/') {
            Self(url)
        } else {
            let mut url = url.clone();
            url.set_path(&format!("{path}/"));
            Self(url)
        }
    }
}

impl<'de> Deserialize<'de> for UrlWithTrailingSlash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let url = Url::deserialize(deserializer)?;
        Ok(url.into())
    }
}

impl FromStr for UrlWithTrailingSlash {
    type Err = url::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Url::parse(s)?.into())
    }
}

impl From<UrlWithTrailingSlash> for Url {
    fn from(value: UrlWithTrailingSlash) -> Self {
        value.0
    }
}

impl Display for UrlWithTrailingSlash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}

impl Redact for UrlWithTrailingSlash {
    fn redact(self) -> Self {
        UrlWithTrailingSlash(self.0.redact())
    }
}
