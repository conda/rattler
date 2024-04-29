use crate::{
    fetch::{FetchRepoDataError, RepoDataNotFoundError},
    gateway::subdir::SubdirClient,
    GatewayError,
};
use bytes::Bytes;
use futures::{FutureExt, TryFutureExt};
use http_cache_semantics::{AfterResponse, BeforeRequest, CachePolicy};
use rattler_conda_types::{Channel, PackageName, PackageRecord, RepoDataRecord};
use rattler_digest::Sha256Hash;
use reqwest::{Response, StatusCode};
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::SystemTime,
};
use tempfile::NamedTempFile;
use tokio::{
    fs::File,
    io::{AsyncReadExt, BufReader},
};
use url::Url;

pub struct ShardedSubdir {
    channel: Channel,
    client: ClientWithMiddleware,
    shard_base_url: Url,
    sharded_repodata: ShardedRepodata,
    cache_dir: PathBuf,
}

/// Magic number that identifies the cache file format.
const MAGIC_NUMBER: &[u8] = b"SHARD-CACHE-V1";

// Fetches the shard index from the url or read it from the cache.
async fn fetch_index(
    client: &ClientWithMiddleware,
    shard_index_url: &Url,
    cache_path: &Path,
) -> Result<ShardedRepodata, GatewayError> {
    async fn from_response(
        cache_path: &Path,
        policy: CachePolicy,
        response: Response,
    ) -> Result<ShardedRepodata, GatewayError> {
        // Read the bytes of the response
        let bytes = response.bytes().await.map_err(FetchRepoDataError::from)?;

        // Decompress the bytes
        let decoded_bytes = Bytes::from(decode_zst_bytes_async(bytes).await?);

        // Write the cache to disk if the policy allows it.
        let cache_fut = if policy.is_storable() {
            write_shard_index_cache(cache_path, policy, decoded_bytes.clone())
                .map_err(FetchRepoDataError::IoError)
                .map_ok(Some)
                .left_future()
        } else {
            // Otherwise delete the file
            tokio::fs::remove_file(cache_path)
                .map_ok_or_else(
                    |e| {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            Ok(None)
                        } else {
                            Err(FetchRepoDataError::IoError(e))
                        }
                    },
                    |_| Ok(None),
                )
                .right_future()
        };

        // Parse the bytes
        let parse_fut = tokio_rayon::spawn(move || rmp_serde::from_slice(&decoded_bytes))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
            .map_err(FetchRepoDataError::IoError);

        // Parse and write the file to disk concurrently
        let (temp_file, sharded_index) = tokio::try_join!(cache_fut, parse_fut)?;

        // Persist the cache if succesfully updated the cache.
        if let Some(temp_file) = temp_file {
            temp_file
                .persist(cache_path)
                .map_err(FetchRepoDataError::from)?;
        }

        Ok(sharded_index)
    }

    // Construct the request to fetch the shard index.
    let request = client
        .get(shard_index_url.clone())
        .build()
        .expect("invalid shard_index request");

    // Try reading the cached file
    if let Ok((cache_header, file)) = read_cached_index(cache_path).await {
        match cache_header
            .policy
            .before_request(&request, SystemTime::now())
        {
            BeforeRequest::Fresh(_) => {
                if let Ok(shard_index) = read_shard_index_from_reader(file).await {
                    tracing::debug!("shard index cache hit");
                    return Ok(shard_index);
                }
            }
            BeforeRequest::Stale {
                request: state_request,
                ..
            } => {
                let request = convert_request(client.clone(), state_request.clone())
                    .expect("failed to create request to check staleness");
                let response = client
                    .execute(request)
                    .await
                    .map_err(FetchRepoDataError::from)?;

                match cache_header.policy.after_response(
                    &state_request,
                    &response,
                    SystemTime::now(),
                ) {
                    AfterResponse::NotModified(_policy, _) => {
                        // The cached file is still valid
                        if let Ok(shard_index) = read_shard_index_from_reader(file).await {
                            tracing::debug!("shard index cache was not modified");
                            // If reading the file failed for some reason we'll just fetch it again.
                            return Ok(shard_index);
                        }
                    }
                    AfterResponse::Modified(policy, _) => {
                        // Close the old file so we can create a new one.
                        drop(file);

                        tracing::debug!("shard index cache has become stale");
                        return from_response(cache_path, policy, response).await;
                    }
                }
            }
        }
    };

    tracing::debug!("fetching fresh shard index");

    // Do a fresh requests
    let response = client
        .execute(
            request
                .try_clone()
                .expect("failed to clone initial request"),
        )
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

    let policy = CachePolicy::new(&request, &response);
    from_response(cache_path, policy, response).await
}

/// Writes the shard index cache to disk.
async fn write_shard_index_cache(
    cache_path: &Path,
    policy: CachePolicy,
    decoded_bytes: Bytes,
) -> std::io::Result<NamedTempFile> {
    let cache_path = cache_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        // Write the header
        let cache_header = rmp_serde::encode::to_vec(&CacheHeader { policy })
            .expect("failed to encode cache header");
        let cache_dir = cache_path
            .parent()
            .expect("the cache path must have a parent");
        std::fs::create_dir_all(cache_dir)?;
        let mut temp_file = tempfile::Builder::new()
            .tempfile_in(cache_dir)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        temp_file.write_all(MAGIC_NUMBER)?;
        temp_file.write_all(&(cache_header.len() as u32).to_le_bytes())?;
        temp_file.write_all(&cache_header)?;
        temp_file.write_all(decoded_bytes.as_ref())?;

        Ok(temp_file)
    })
    .map_err(|e| match e.try_into_panic() {
        Ok(payload) => std::panic::resume_unwind(payload),
        Err(e) => std::io::Error::new(std::io::ErrorKind::Other, e),
    })
    .await?
}

/// Converts from a `http::request::Parts` into a `reqwest::Request`.
fn convert_request(
    client: ClientWithMiddleware,
    parts: http::request::Parts,
) -> Result<reqwest::Request, reqwest::Error> {
    client
        .request(
            parts.method,
            Url::from_str(&parts.uri.to_string()).expect("uris should be the same"),
        )
        .headers(parts.headers)
        .version(parts.version)
        .build()
}

/// Read the shard index from a reader and deserialize it.
async fn read_shard_index_from_reader(
    mut reader: BufReader<File>,
) -> std::io::Result<ShardedRepodata> {
    // Read the file to memory
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;

    // Deserialize the bytes
    tokio_rayon::spawn(move || rmp_serde::from_slice(&bytes))
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// Cache information stored at the start of the cache file.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheHeader {
    pub policy: CachePolicy,
}

/// Try reading the cache file from disk.
async fn read_cached_index(cache_path: &Path) -> std::io::Result<(CacheHeader, BufReader<File>)> {
    // Open the file for reading
    let file = File::open(cache_path).await?;
    let mut reader = BufReader::new(file);

    // Read the magic from the file
    let mut magic_number = [0; MAGIC_NUMBER.len()];
    reader.read_exact(&mut magic_number).await?;
    if magic_number != MAGIC_NUMBER {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid magic number",
        ));
    }

    // Read the length of the header
    let header_length = reader.read_u32_le().await? as usize;

    // Read the header from the file
    let mut header_bytes = vec![0; header_length];
    reader.read_exact(&mut header_bytes).await?;

    // Deserialize the header
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
        let sharded_repodata =
            fetch_index(&client, &repodata_shards_url, &sharded_repodata_path).await?;

        // Determine the cache directory and make sure it exists.
        let cache_dir = cache_dir.join("shards-v1");
        tokio::fs::create_dir_all(&cache_dir)
            .await
            .map_err(FetchRepoDataError::IoError)?;

        Ok(Self {
            channel,
            client,
            shard_base_url,
            sharded_repodata,
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
        let shard_cache_path = self.cache_dir.join(format!("{shard:x}.msgpack"));

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
            .join(&format!("shards/{shard:x}.msgpack.zst"))
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
