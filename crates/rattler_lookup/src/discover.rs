//! Finding the index of a subdir through its repodata.
//!
//! A channel that publishes a lookup index points at the manifest of every
//! indexed subdir with the `info.lookup_url` field of the subdir's
//! repodata, like `shards_base_url` of
//! [CEP 16](https://github.com/conda/ceps/blob/main/cep-0016.md).

use std::fmt;

use bytes::Bytes;
use reqwest::{StatusCode, header};
use reqwest_middleware::ClientWithMiddleware;
use serde::{
    Deserialize,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor},
};
use url::Url;

use crate::{LookupError, Result, manifest::directory_url};

/// How many bytes of a repodata file are read to find `info.lookup_url`.
///
/// `info` comes first in the repodata files of every implementation we know of,
/// so its prefix is all that has to be downloaded. If the field is not in the
/// prefix, the whole file is read.
const PREFIX_SIZE: u64 = 64 * 1024;

/// The url of the manifest of the lookup index of `subdir`, as advertised by the
/// channel, or `None` if the channel publishes no index for that subdir.
///
/// The sharded repodata index is consulted first (it is the smaller file), the
/// `repodata.json` of the subdir second.
pub async fn discover_manifest_url(
    channel_base: &Url,
    subdir: &str,
    client: &ClientWithMiddleware,
) -> Result<Option<Url>> {
    let base = directory_url(channel_base)?;
    let shards = base.join(&format!("{subdir}/repodata_shards.msgpack.zst"))?;
    if let Some(url) = lookup_url(&shards, Encoding::MsgpackZst, client).await? {
        return Ok(Some(url));
    }

    let zipped = base.join(&format!("{subdir}/repodata.json.zst"))?;
    if let Some(url) = lookup_url(&zipped, Encoding::JsonZst, client).await? {
        return Ok(Some(url));
    }

    let repodata = base.join(&format!("{subdir}/repodata.json"))?;
    lookup_url(&repodata, Encoding::Json, client).await
}

/// How a repodata file is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    /// `repodata.json`
    Json,
    /// `repodata.json.zst`
    JsonZst,
    /// `repodata_shards.msgpack.zst`
    MsgpackZst,
}

/// Reads `info.lookup_url` of one repodata file and resolves it relative to
/// that file. `None` if the file or the field does not exist.
async fn lookup_url(
    url: &Url,
    encoding: Encoding,
    client: &ClientWithMiddleware,
) -> Result<Option<Url>> {
    let Some(body) = fetch(url, Some(PREFIX_SIZE), client).await? else {
        return Ok(None);
    };
    let found = match parse(&body.bytes, encoding) {
        Ok(found) => found,
        // The prefix may have cut the file off before `info`.
        Err(err) if body.partial => {
            tracing::debug!("re-reading all of {url}: {err}");
            let Some(body) = fetch(url, None, client).await? else {
                return Ok(None);
            };
            parse(&body.bytes, encoding).map_err(|source| LookupError::InvalidRepodata {
                url: Box::new(url.clone()),
                source,
            })?
        }
        Err(source) => {
            return Err(LookupError::InvalidRepodata {
                url: Box::new(url.clone()),
                source,
            });
        }
    };
    found
        .map(|found| url.join(&found))
        .transpose()
        .map_err(Into::into)
}

/// The (possibly partial) body of a file.
struct Body {
    bytes: Bytes,
    /// Whether this is only the first [`PREFIX_SIZE`] bytes of the file.
    partial: bool,
}

/// Fetches (part of) a file, or `None` if it does not exist.
async fn fetch(
    url: &Url,
    prefix: Option<u64>,
    client: &ClientWithMiddleware,
) -> Result<Option<Body>> {
    let mut request = client.get(url.clone());
    if let Some(prefix) = prefix {
        request = request.header(header::RANGE, format!("bytes=0-{}", prefix - 1));
    }
    let response = request.send().await.map_err(|source| LookupError::Http {
        url: Box::new(url.clone()),
        source,
    })?;
    if matches!(
        response.status(),
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN | StatusCode::GONE
    ) {
        return Ok(None);
    }
    let partial = response.status() == StatusCode::PARTIAL_CONTENT;
    let response = response
        .error_for_status()
        .map_err(|err| LookupError::HttpStatus {
            url: Box::new(url.clone()),
            status: err.status().unwrap_or_default(),
        })?;
    let bytes = response.bytes().await.map_err(|err| LookupError::Http {
        url: Box::new(url.clone()),
        source: err.into(),
    })?;
    Ok(Some(Body { bytes, partial }))
}

/// Deserializes `info.lookup_url` out of (the beginning of) a repodata
/// file.
///
/// Deserializing stops as soon as `info` has been read, so only the beginning of
/// a (possibly truncated) repodata file is needed. Both `serde_json` and
/// `rmp_serde` report an error when the rest of the file is left unread — that
/// error is expected and ignored once `info` was found.
fn parse(bytes: &[u8], encoding: Encoding) -> std::result::Result<Option<String>, BoxedError> {
    let mut info: Option<Info> = None;
    let seed = InfoSeed(&mut info);
    let result = match encoding {
        Encoding::Json => seed
            .deserialize(&mut serde_json::Deserializer::from_slice(bytes))
            .map_err(BoxedError::from),
        Encoding::JsonZst => seed
            .deserialize(&mut serde_json::Deserializer::from_reader(
                zstd::Decoder::new(bytes)?,
            ))
            .map_err(BoxedError::from),
        Encoding::MsgpackZst => seed
            .deserialize(&mut rmp_serde::Deserializer::new(zstd::Decoder::new(
                bytes,
            )?))
            .map_err(BoxedError::from),
    };
    match info {
        Some(info) => Ok(info.lookup_url),
        None => result.map(|()| None),
    }
}

type BoxedError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Default, Deserialize)]
struct Info {
    #[serde(default)]
    lookup_url: Option<String>,
}

/// Reads the `info` object of a repodata file into the given slot.
struct InfoSeed<'a>(&'a mut Option<Info>);

impl<'de> de::DeserializeSeed<'de> for InfoSeed<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for InfoSeed<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a repodata file")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if key == "info" {
                let info: Info = map.next_value()?;
                *self.0 = Some(info);
                // Stop here: the rest of the file is not needed and may not even
                // have been downloaded.
                return Ok(());
            }
            map.next_value::<IgnoredAny>()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zstd(bytes: &[u8]) -> Vec<u8> {
        zstd::encode_all(bytes, 3).unwrap()
    }

    #[test]
    fn reads_json() {
        let json = br#"{"info":{"subdir":"noarch","lookup_url":"./lookup/manifest.json"},
            "packages":{}}"#;
        assert_eq!(
            parse(json, Encoding::Json).unwrap().as_deref(),
            Some("./lookup/manifest.json")
        );
        assert_eq!(
            parse(&zstd(json), Encoding::JsonZst).unwrap().as_deref(),
            Some("./lookup/manifest.json")
        );
    }

    #[test]
    fn a_channel_without_an_index_has_no_url() {
        let json = br#"{"info":{"subdir":"noarch"},"packages":{}}"#;
        assert_eq!(parse(json, Encoding::Json).unwrap(), None);
        // Not even an `info` object.
        assert_eq!(parse(br#"{"packages":{}}"#, Encoding::Json).unwrap(), None);
    }

    #[test]
    fn stops_reading_after_info() {
        // Everything after `info` is cut off, as a range request would.
        let json = br#"{"info":{"lookup_url":"m.json"},"packages":{"a":{"version":"1."#;
        assert_eq!(
            parse(json, Encoding::Json).unwrap().as_deref(),
            Some("m.json")
        );
        let mut truncated = zstd(json);
        truncated.truncate(truncated.len() - 4);
        assert_eq!(
            parse(&truncated, Encoding::JsonZst).unwrap().as_deref(),
            Some("m.json")
        );
    }

    #[test]
    fn reads_msgpack() {
        #[derive(serde::Serialize)]
        struct Shards<'a> {
            info: std::collections::BTreeMap<&'a str, &'a str>,
            shards: std::collections::BTreeMap<&'a str, &'a str>,
        }
        let shards = Shards {
            info: [("lookup_url", "./lookup/manifest.json")]
                .into_iter()
                .collect(),
            shards: [("a", "b")].into_iter().collect(),
        };
        let bytes = zstd(&rmp_serde::to_vec_named(&shards).unwrap());
        assert_eq!(
            parse(&bytes, Encoding::MsgpackZst).unwrap().as_deref(),
            Some("./lookup/manifest.json")
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"not json", Encoding::Json).is_err());
    }
}
