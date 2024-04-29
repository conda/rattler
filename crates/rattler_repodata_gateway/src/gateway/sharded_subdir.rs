use crate::fetch::{FetchRepoDataError, RepoDataNotFoundError};
use crate::gateway::subdir::SubdirClient;
use crate::GatewayError;
use chrono::{DateTime, Utc};
use futures::TryFutureExt;
use rattler_conda_types::{Channel, PackageName, PackageRecord, RepoDataRecord};
use rattler_digest::Sha256Hash;
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::io::AsyncReadExt;
use url::Url;

pub struct ShardedSubdir {
    channel: Channel,
    client: ClientWithMiddleware,
    shard_base_url: Url,
    sharded_repodata: ShardedRepodata,
    cache_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheHeader {
    pub etag: Option<String>,
    pub last_modified: Option<DateTime<Utc>>,
}

/// Magic number that identifies the cache file format.
const MAGIC_NUMBER: &[u8] = b"SHARD-CACHE-V1";

/// Write the shard index cache to disk.
async fn write_cache(
    cache_file: &Path,
    cache_header: CacheHeader,
    cache_data: &[u8],
) -> Result<(), std::io::Error> {
    let cache_header_bytes = rmp_serde::to_vec(&cache_header).unwrap();
    let header_length = cache_header_bytes.len() as usize;
    // write it as 4 bytes
    let content = [
        MAGIC_NUMBER,
        &header_length.to_le_bytes(),
        &cache_header_bytes,
        cache_data,
    ]
    .concat();
    tokio::fs::write(&cache_file, content).await
}

/// Read the cache header - returns the cache header and the reader that can be
/// used to read the rest of the file.
async fn read_cache_header(
    cache_file: &Path,
) -> Result<(CacheHeader, tokio::io::BufReader<tokio::fs::File>), std::io::Error> {
    let cache_data = tokio::fs::File::open(&cache_file).await?;
    let mut reader = tokio::io::BufReader::new(cache_data);
    let mut magic_number = [0; MAGIC_NUMBER.len()];
    reader.read_exact(&mut magic_number).await?;
    if magic_number != MAGIC_NUMBER {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid magic number",
        ));
    }

    let mut header_length_bytes = [0; 8];
    reader.read_exact(&mut header_length_bytes).await?;
    let header_length = usize::from_le_bytes(header_length_bytes);
    let mut header_bytes = vec![0; header_length];
    reader.read_exact(&mut header_bytes).await?;
    let cache_header = rmp_serde::from_slice::<CacheHeader>(&header_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    Ok((cache_header, reader))
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

        let cache_key = crate::utils::url_to_cache_filename(&repodata_shards_url);
        let sharded_repodata_path = cache_dir.join(format!("{cache_key}.shard-cache-v1"));

        let mut cache_data = None;
        if sharded_repodata_path.exists() {
            // split the header from the sharded repodata
            let mut result = None;
            match read_cache_header(&sharded_repodata_path).await {
                Ok((cache_header, file)) => {
                    result = Some((cache_header, file));
                }
                Err(e) => {
                    tracing::info!("failed to read cache header: {:?}", e);
                    // remove the file and try to fetch it again, ignore any error here
                    tokio::fs::remove_file(&sharded_repodata_path).await.ok();
                }
            }

            if let Some((cache_header, mut file)) = result {
                // Cache times out after 1 hour
                let mut rest = Vec::new();
                // parse the last_modified header
                if let Some(last_modified) = &cache_header.last_modified {
                    let now: DateTime<Utc> = SystemTime::now().into();
                    let elapsed = now - last_modified;
                    if elapsed > chrono::Duration::hours(1) {
                        // insert the etag
                        cache_data = Some((cache_header, file));
                    } else {
                        tracing::info!("Using cached sharded repodata - cache still valid");
                        match file.read_to_end(&mut rest).await {
                            Ok(_) => {
                                let sharded_repodata = rmp_serde::from_slice(&rest).unwrap();
                                return Ok(Self {
                                    channel,
                                    client,
                                    sharded_repodata,
                                    shard_base_url,
                                    cache_dir,
                                });
                            }
                            Err(e) => {
                                tracing::info!("failed to read cache data: {:?}", e);
                                // remove the file and try to fetch it again, ignore any error here
                                tokio::fs::remove_file(&sharded_repodata_path).await.ok();
                            }
                        }
                    }
                }
            }
        }

        let response = client
            .get(repodata_shards_url.clone())
            .send()
            .await
            .map_err(FetchRepoDataError::from)?;

        // Check if the response was successful.
        if response.status() == StatusCode::NOT_FOUND {
            return Err(GatewayError::FetchRepoDataError(
                FetchRepoDataError::NotFound(RepoDataNotFoundError::from(
                    response.error_for_status().unwrap_err(),
                )),
            ));
        };

        if let Some((cache_header, mut file)) = cache_data {
            let found_etag = response.headers().get("etag").and_then(|v| v.to_str().ok());

            if found_etag == cache_header.etag.as_deref() {
                // The cached file is up to date
                tracing::info!("Using cached sharded repodata - etag match");
                let mut rest = Vec::new();
                match file.read_to_end(&mut rest).await {
                    Ok(_) => {
                        let sharded_repodata = rmp_serde::from_slice(&rest).unwrap();
                        return Ok(Self {
                            channel,
                            client,
                            sharded_repodata,
                            shard_base_url,
                            cache_dir,
                        });
                    }
                    Err(e) => {
                        tracing::info!("failed to read cache data: {:?}", e);
                        // remove the file and try to fetch it again, ignore any error here
                        tokio::fs::remove_file(&sharded_repodata_path).await.ok();
                    }
                }
            }
        }

        let response = response
            .error_for_status()
            .map_err(FetchRepoDataError::from)?;

        let cache_header = CacheHeader {
            etag: response
                .headers()
                .get("etag")
                .map(|v| v.to_str().unwrap().to_string()),
            last_modified: response
                .headers()
                .get("last-modified")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| DateTime::parse_from_rfc2822(v).ok())
                .map(|v| v.with_timezone(&Utc)),
        };

        // Parse the sharded repodata from the response
        let sharded_repodata_compressed_bytes =
            response.bytes().await.map_err(FetchRepoDataError::from)?;
        let sharded_repodata_bytes =
            decode_zst_bytes_async(sharded_repodata_compressed_bytes).await?;

        // write the sharded repodata to disk
        write_cache(
            &sharded_repodata_path,
            cache_header,
            &sharded_repodata_bytes,
        )
        .await
        .map_err(|e| {
            FetchRepoDataError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e))
        })?;

        let sharded_repodata = tokio_rayon::spawn(move || {
            rmp_serde::from_slice::<ShardedRepodata>(&sharded_repodata_bytes)
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
        .map_err(FetchRepoDataError::IoError)?;

        // Determine the cache directory and make sure it exists.
        let cache_dir = cache_dir.join("shards-v1");
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
