use crate::fetch::{FetchRepoDataError, RepoDataNotFoundError};
use crate::gateway::subdir::SubdirClient;
use crate::GatewayError;
use futures::TryFutureExt;
use rattler_conda_types::{Channel, PackageName, PackageRecord, RepoDataRecord};
use rattler_digest::Sha256Hash;
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;

pub struct ShardedSubdir {
    channel: Channel,
    client: ClientWithMiddleware,
    shard_base_url: Url,
    sharded_repodata: ShardedRepodata,
    cache_dir: PathBuf,
}

impl ShardedSubdir {
    pub async fn new(
        _channel: Channel,
        subdir: String,
        client: ClientWithMiddleware,
        cache_dir: PathBuf,
    ) -> Result<Self, GatewayError> {
        // TODO: our sharded index only serves conda-forge so we simply override it.
        let channel =
            Channel::from_url(Url::parse("https://conda.anaconda.org/conda-forge").unwrap());

        let shard_base_url =
            Url::parse(&format!("https://fast.prefiks.dev/conda-forge/{subdir}/")).unwrap();

        // Fetch the sharded repodata from the remote server
        let repodata_shards_url = shard_base_url
            .join("repodata_shards.msgpack.zst")
            .expect("invalid shard base url");
        let response = client
            .get(repodata_shards_url.clone())
            .send()
            .await
            .map_err(FetchRepoDataError::from)?;

        // Check if the response was succesfull.
        if response.status() == StatusCode::NOT_FOUND {
            return Err(GatewayError::FetchRepoDataError(
                FetchRepoDataError::NotFound(RepoDataNotFoundError::from(
                    response.error_for_status().unwrap_err(),
                )),
            ));
        };

        let response = response
            .error_for_status()
            .map_err(FetchRepoDataError::from)?;

        // Parse the sharded repodata from the response
        let sharded_repodata_compressed_bytes =
            response.bytes().await.map_err(FetchRepoDataError::from)?;
        let sharded_repodata_bytes =
            decode_zst_bytes_async(sharded_repodata_compressed_bytes).await?;
        let sharded_repodata = tokio_rayon::spawn(move || {
            rmp_serde::from_slice::<ShardedRepodata>(&sharded_repodata_bytes)
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
        .map_err(FetchRepoDataError::IoError)?;

        // Determine the cache directory and make sure it exists.
        let cache_dir = cache_dir.join("shards_v1");
        tokio::fs::create_dir_all(&cache_dir)
            .await
            .map_err(FetchRepoDataError::IoError)?;

        Ok(Self {
            channel,
            client,
            sharded_repodata,
            shard_base_url,
            cache_dir,
        })
    }
}

#[async_trait::async_trait]
impl SubdirClient for ShardedSubdir {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
    ) -> Result<Arc<[RepoDataRecord]>, GatewayError> {
        // Find the shard that contains the package
        let Some(shard) = self.sharded_repodata.shards.get(name.as_normalized()) else {
            return Ok(vec![].into());
        };

        // Check if we already have the shard in the cache.
        let shard_cache_path = self.cache_dir.join(&format!("{:x}.msgpack", shard));

        // Read the cached shard
        match tokio::fs::read(&shard_cache_path).await {
            Ok(cached_bytes) => {
                // Decode the cached shard
                return parse_records(
                    cached_bytes,
                    self.channel.canonical_name(),
                    self.sharded_repodata.info.base_url.clone(),
                )
                .await
                .map(Arc::from);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // The file is missing from the cache, we need to download it.
            }
            Err(err) => return Err(FetchRepoDataError::IoError(err).into()),
        }

        // Download the shard
        let shard_url = self
            .shard_base_url
            .join(&format!("shards/{:x}.msgpack.zst", shard))
            .expect("invalid shard url");

        let shard_response = self
            .client
            .get(shard_url.clone())
            .send()
            .await
            .and_then(|r| r.error_for_status().map_err(Into::into))
            .map_err(FetchRepoDataError::from)?;

        let shard_bytes = shard_response
            .bytes()
            .await
            .map_err(FetchRepoDataError::from)?;

        let shard_bytes = decode_zst_bytes_async(shard_bytes).await?;

        // Create a future to write the cached bytes to disk
        let write_to_cache_fut = tokio::fs::write(&shard_cache_path, shard_bytes.clone())
            .map_err(FetchRepoDataError::IoError)
            .map_err(GatewayError::from);

        // Create a future to parse the records from the shard
        let parse_records_fut = parse_records(
            shard_bytes,
            self.channel.canonical_name(),
            self.sharded_repodata.info.base_url.clone(),
        );

        // Await both futures concurrently.
        let (_, records) = tokio::try_join!(write_to_cache_fut, parse_records_fut)?;

        Ok(records.into())
    }
}

async fn decode_zst_bytes_async<R: AsRef<[u8]> + Send + 'static>(
    bytes: R,
) -> Result<Vec<u8>, GatewayError> {
    tokio_rayon::spawn(move || match zstd::decode_all(bytes.as_ref()) {
        Ok(decoded) => Ok(decoded),
        Err(err) => Err(GatewayError::IoError(
            "failed to decode zstd shard".to_string(),
            err,
        )),
    })
    .await
}

async fn parse_records<R: AsRef<[u8]> + Send + 'static>(
    bytes: R,
    channel_name: String,
    base_url: Url,
) -> Result<Vec<RepoDataRecord>, GatewayError> {
    tokio_rayon::spawn(move || {
        // let shard = serde_json::from_slice::<Shard>(bytes.as_ref()).map_err(std::io::Error::from)?;
        let shard = rmp_serde::from_slice::<Shard>(bytes.as_ref())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
            .map_err(FetchRepoDataError::IoError)?;
        let packages =
            itertools::chain(shard.packages.into_iter(), shard.packages_conda.into_iter());
        let base_url = add_trailing_slash(&base_url);
        Ok(packages
            .map(|(file_name, package_record)| RepoDataRecord {
                url: base_url
                    .join(&file_name)
                    .expect("filename is not a valid url"),
                channel: channel_name.clone(),
                package_record,
                file_name,
            })
            .collect())
    })
    .await
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardedRepodata {
    pub info: ShardedSubdirInfo,
    /// The individual shards indexed by package name.
    pub shards: HashMap<String, Sha256Hash>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shard {
    pub packages: HashMap<String, PackageRecord>,

    #[serde(rename = "packages.conda", default)]
    pub packages_conda: HashMap<String, PackageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardedSubdirInfo {
    /// The name of the subdirectory
    pub subdir: String,

    /// The base url of the subdirectory. This is the location where the actual
    /// packages are stored.
    pub base_url: Url,
}
