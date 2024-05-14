//! A helper module to serialize a field of type `UrlOrPath` as either
//! ```yaml
//! path: ./path/to/file
//! ```
//! or
//! ```yaml
//! url: https://some_url.com
//! ```

use crate::UrlOrPath;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{borrow::Cow, path::PathBuf};
use url::Url;

#[derive(Serialize, Deserialize)]
struct RawUrlOrPath<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Cow<'a, Url>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<Cow<'a, PathBuf>>,
}

pub fn serialize<S>(value: &UrlOrPath, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let raw = match value {
        UrlOrPath::Url(url) => RawUrlOrPath {
            url: Some(Cow::Borrowed(url)),
            path: None,
        },
        UrlOrPath::Path(path) => RawUrlOrPath {
            url: None,
            path: Some(Cow::Borrowed(path)),
        },
    };

    raw.serialize(serializer)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<UrlOrPath, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = RawUrlOrPath::<'de>::deserialize(deserializer)?;
    match (raw.url, raw.path) {
        (Some(url), None) => Ok(UrlOrPath::Url(url.into_owned())),
        (None, Some(path)) => Ok(UrlOrPath::Path(path.into_owned())),
        _ => Err(serde::de::Error::custom("expected either a url or a path")),
    }
}
