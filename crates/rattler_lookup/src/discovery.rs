//! Finding the manifest of a subdir through the `lookup_url` of its repodata.
//!
//! The `info` object of `repodata.json` and of the sharded repodata index
//! `repodata_shards.msgpack.zst` carries `lookup_url`, the URL of the
//! subdir's `manifest.json`, if the channel publishes a lookup index.

use std::io::Read;

use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;

use crate::{Location, LookupError, fetch::fetch_optional};

/// The sharded repodata index of a subdir.
pub const REPODATA_SHARDS: &str = "repodata_shards.msgpack.zst";
/// The repodata of a subdir.
pub const REPODATA: &str = "repodata.json";

#[derive(Deserialize)]
struct InfoProbe {
    #[serde(default)]
    lookup_url: Option<String>,
}

#[derive(Deserialize)]
struct RepodataProbe {
    #[serde(default)]
    info: Option<InfoProbe>,
}

#[derive(Deserialize)]
struct ShardsProbe {
    info: InfoProbe,
}

/// Finds the manifest of `subdir` of a channel (its base URL or directory).
///
/// The sharded repodata index is tried first, since it is small, then
/// `repodata.json.zst` and `repodata.json`. Returns `None` if the subdir has
/// no repodata or its repodata has no `lookup_url`.
pub async fn discover_manifest(
    channel: &Location,
    subdir: &str,
    client: &ClientWithMiddleware,
) -> Result<Option<Location>, LookupError> {
    let shards = channel.join(&format!("{subdir}/{REPODATA_SHARDS}"));
    if let Some(bytes) = fetch_optional(&shards, client).await? {
        let decoded = decompress(&bytes, &shards)?;
        let probe: ShardsProbe = rmp_serde::from_slice(&decoded).map_err(|e| {
            LookupError::invalid_file(&shards, format!("invalid sharded repodata: {e}"))
        })?;
        return Ok(probe
            .info
            .lookup_url
            .map(|url| shards.resolve_lookup_url(&url)));
    }

    let zst = channel.join(&format!("{subdir}/{REPODATA}.zst"));
    if let Some(bytes) = fetch_optional(&zst, client).await? {
        let decoded = decompress(&bytes, &zst)?;
        return lookup_url_of_repodata(&decoded, &zst);
    }

    let json = channel.join(&format!("{subdir}/{REPODATA}"));
    match fetch_optional(&json, client).await? {
        Some(bytes) => lookup_url_of_repodata(&bytes, &json),
        None => Ok(None),
    }
}

fn lookup_url_of_repodata(
    bytes: &[u8],
    location: &Location,
) -> Result<Option<Location>, LookupError> {
    let probe: RepodataProbe = serde_json::from_slice(bytes)
        .map_err(|e| LookupError::invalid_file(location, format!("invalid repodata: {e}")))?;
    Ok(probe
        .info
        .and_then(|info| info.lookup_url)
        .map(|url| location.resolve_lookup_url(&url)))
}

fn decompress(bytes: &[u8], location: &Location) -> Result<Vec<u8>, LookupError> {
    let mut decoded = Vec::new();
    zstd::Decoder::new(bytes)
        .and_then(|mut decoder| decoder.read_to_end(&mut decoded))
        .map_err(|e| LookupError::io(location, e))?;
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lookup_url_from_repodata() {
        let location = Location::parse("https://x.org/c/noarch/repodata.json");
        let json =
            br#"{"info":{"subdir":"noarch","lookup_url":"./lookup/manifest.json"},"packages":{}}"#;
        assert_eq!(
            lookup_url_of_repodata(json, &location).unwrap(),
            Some(Location::parse(
                "https://x.org/c/noarch/lookup/manifest.json"
            ))
        );
        let json = br#"{"info":{"subdir":"noarch"},"packages":{}}"#;
        assert_eq!(lookup_url_of_repodata(json, &location).unwrap(), None);
        let json = br#"{"packages":{}}"#;
        assert_eq!(lookup_url_of_repodata(json, &location).unwrap(), None);
    }
}
