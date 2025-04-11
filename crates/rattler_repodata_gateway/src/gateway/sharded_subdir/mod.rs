use std::borrow::Cow;

use cfg_if::cfg_if;
use rattler_conda_types::{ChannelUrl, RepoDataRecord, Shard};
use rattler_redaction::Redact;
use url::Url;

use crate::{fetch::FetchRepoDataError, GatewayError};

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        mod wasm;
        pub use wasm::ShardedSubdir;
    } else {
        mod tokio;
        pub use tokio::ShardedSubdir;
    }
}

/// Returns the URL with a trailing slash if it doesn't already have one.
fn add_trailing_slash(url: &Url) -> Cow<'_, Url> {
    let path = url.path();
    if path.ends_with('/') {
        Cow::Borrowed(url)
    } else {
        let mut url = url.clone();
        url.set_path(&format!("{path}/"));
        Cow::Owned(url)
    }
}

async fn decode_zst_bytes_async<R: AsRef<[u8]> + Send + 'static>(
    bytes: R,
) -> Result<Vec<u8>, GatewayError> {
    let decode = move || match zstd::decode_all(bytes.as_ref()) {
        Ok(decoded) => Ok(decoded),
        Err(err) => Err(GatewayError::IoError(
            "failed to decode zstd shard".to_string(),
            err,
        )),
    };

    #[cfg(target_arch = "wasm32")]
    return decode();

    #[cfg(not(target_arch = "wasm32"))]
    simple_spawn_blocking::tokio::run_blocking_task(decode).await
}

async fn parse_records<R: AsRef<[u8]> + Send + 'static>(
    bytes: R,
    channel_base_url: ChannelUrl,
    base_url: Url,
) -> Result<Vec<RepoDataRecord>, GatewayError> {
    let parse = move || {
        // let shard =
        // serde_json::from_slice::<Shard>(bytes.as_ref()).
        // map_err(std::io::Error::from)?;
        let shard = rmp_serde::from_slice::<Shard>(bytes.as_ref())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
            .map_err(FetchRepoDataError::IoError)?;
        let packages =
            itertools::chain(shard.packages.into_iter(), shard.conda_packages.into_iter())
                .filter(|(name, _record)| !shard.removed.contains(name));
        Ok(packages
            .map(|(file_name, package_record)| RepoDataRecord {
                url: base_url
                    .join(&file_name)
                    .expect("filename is not a valid url"),
                channel: Some(channel_base_url.url().clone().redact().to_string()),
                package_record,
                file_name,
            })
            .collect())
    };

    #[cfg(target_arch = "wasm32")]
    return parse();

    #[cfg(not(target_arch = "wasm32"))]
    simple_spawn_blocking::tokio::run_blocking_task(parse).await
}
