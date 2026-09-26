//! Finding the manifest of a subdir through the `lookup_url` of its sharded
//! repodata index.
//!
//! The `info` object of `repodata_shards.msgpack.zst` carries `lookup_url`,
//! the URL of the subdir's `manifest.json`, if the channel publishes a lookup
//! index. (`repodata.json` carries it too, but it is far too large to fetch
//! just for that, so a channel is expected to have a sharded index.)

use std::io::Read;

use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;

use crate::{Location, LookupError, fetch::fetch_optional};

/// The sharded repodata index of a subdir.
pub const REPODATA_SHARDS: &str = "repodata_shards.msgpack.zst";

#[derive(Deserialize)]
struct InfoProbe {
    #[serde(default)]
    lookup_url: Option<String>,
}

#[derive(Deserialize)]
struct ShardsProbe {
    info: InfoProbe,
}

/// Finds the manifest of `subdir` of a channel (its base URL or directory)
/// through the `lookup_url` of its sharded repodata index.
///
/// Returns `None` if the subdir has no sharded repodata index or the index
/// has no `lookup_url`.
pub async fn discover_manifest(
    channel: &Location,
    subdir: &str,
    client: &ClientWithMiddleware,
) -> Result<Option<Location>, LookupError> {
    let shards = channel.join(&format!("{subdir}/{REPODATA_SHARDS}"));
    let Some(bytes) = fetch_optional(&shards, client).await? else {
        return Ok(None);
    };
    let decoded = decompress(&bytes, &shards)?;
    let probe: ShardsProbe = rmp_serde::from_slice(&decoded).map_err(|e| {
        LookupError::invalid_file(&shards, format!("invalid sharded repodata: {e}"))
    })?;
    Ok(probe
        .info
        .lookup_url
        .map(|url| shards.resolve_lookup_url(&url)))
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
    fn reads_lookup_url_from_shards() {
        let shards = serde_json::json!({
            "info": {"subdir": "noarch", "shards_base_url": "./shards/", "lookup_url": "./lookup/manifest.json"},
            "shards": {},
        });
        let probe: ShardsProbe =
            rmp_serde::from_slice(&rmp_serde::to_vec_named(&shards).unwrap()).unwrap();
        assert_eq!(
            probe.info.lookup_url.as_deref(),
            Some("./lookup/manifest.json")
        );
        assert_eq!(
            Location::parse("https://x.org/c/noarch/repodata_shards.msgpack.zst")
                .resolve_lookup_url("./lookup/manifest.json")
                .to_string(),
            "https://x.org/c/noarch/lookup/manifest.json"
        );
        let shards = serde_json::json!({"info": {"subdir": "noarch"}, "shards": {}});
        let probe: ShardsProbe =
            rmp_serde::from_slice(&rmp_serde::to_vec_named(&shards).unwrap()).unwrap();
        assert_eq!(probe.info.lookup_url, None);
    }
}
