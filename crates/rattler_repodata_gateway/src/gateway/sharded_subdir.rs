use crate::gateway::PendingOrFetched;
use crate::utils::url_to_cache_filename;
use crate::{
    fetch::{FetchRepoDataError, RepoDataNotFoundError},
    gateway::subdir::SubdirClient,
    GatewayError,
};
use bytes::Bytes;
use chrono::{DateTime, TimeDelta, Utc};
use futures::{FutureExt, TryFutureExt};
use http::header::CACHE_CONTROL;
use http::{HeaderMap, HeaderValue, Method, Uri};
use http_cache_semantics::{AfterResponse, BeforeRequest, CachePolicy, RequestLike};
use itertools::Either;
use parking_lot::Mutex;
use rattler_conda_types::{Channel, PackageName, PackageRecord, RepoDataRecord};
use rattler_digest::Sha256Hash;
use reqwest::{Response, StatusCode};
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::ops::Add;
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
    channel_base_url: Url,
    token_client: TokenClient,
    sharded_repodata: ShardedRepodata,
    cache_dir: PathBuf,
}

/// Magic number that identifies the cache file format.
const MAGIC_NUMBER: &[u8] = b"SHARD-CACHE-V1";
const REPODATA_SHARDS_FILENAME: &str = "repodata_shards.msgpack.zst";

struct SimpleRequest {
    uri: Uri,
    method: Method,
    headers: HeaderMap,
}

impl SimpleRequest {
    pub fn get(url: &Url) -> Self {
        Self {
            uri: Uri::from_str(url.as_str()).expect("failed to convert Url to Uri"),
            method: Method::GET,
            headers: HeaderMap::default(),
        }
    }
}

impl RequestLike for SimpleRequest {
    fn method(&self) -> &Method {
        &self.method
    }

    fn uri(&self) -> Uri {
        self.uri.clone()
    }

    fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    fn is_same_uri(&self, other: &Uri) -> bool {
        &self.uri() == other
    }
}

// Fetches the shard index from the url or read it from the cache.
async fn fetch_index(
    client: ClientWithMiddleware,
    channel_base_url: &Url,
    token_client: &TokenClient,
    cache_dir: &Path,
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

    // Fetch the sharded repodata from the remote server
    let canonical_shards_url = channel_base_url
        .join(REPODATA_SHARDS_FILENAME)
        .expect("invalid shard base url");

    let cache_file_name = format!(
        "{}.shards-cache-v1",
        url_to_cache_filename(&canonical_shards_url)
    );
    let cache_path = cache_dir.join(cache_file_name);

    let canonical_request = SimpleRequest::get(&canonical_shards_url);

    // Try reading the cached file
    if let Ok((cache_header, file)) = read_cached_index(&cache_path).await {
        match cache_header
            .policy
            .before_request(&canonical_request, SystemTime::now())
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
                // Get the token from the token client
                let token = token_client.get_token().await?;

                // Determine the actual URL to use for the request
                let shards_url = token
                    .shard_base_url
                    .as_ref()
                    .unwrap_or(channel_base_url)
                    .join(REPODATA_SHARDS_FILENAME)
                    .expect("invalid shard base url");

                // Construct the actual request that we will send
                let mut request = client
                    .get(shards_url)
                    .headers(state_request.headers().clone())
                    .build()
                    .expect("failed to build request for shard index");
                token.add_to_headers(request.headers_mut());

                // Send the request
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
                        return from_response(&cache_path, policy, response).await;
                    }
                }
            }
        }
    };

    tracing::debug!("fetching fresh shard index");

    // Get the token from the token client
    let token = token_client.get_token().await?;

    // Determine the actual URL to use for the request
    let shards_url = token
        .shard_base_url
        .as_ref()
        .unwrap_or(channel_base_url)
        .join(REPODATA_SHARDS_FILENAME)
        .expect("invalid shard base url");

    // Construct the actual request that we will send
    let mut request = client
        .get(shards_url)
        .build()
        .expect("failed to build request for shard index");
    token.add_to_headers(request.headers_mut());

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

    let policy = CachePolicy::new(&canonical_request, &response);
    from_response(&cache_path, policy, response).await
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

struct TokenClient {
    client: ClientWithMiddleware,
    token_base_url: Url,
    token: Arc<Mutex<PendingOrFetched<Option<Arc<Token>>>>>,
}

impl TokenClient {
    pub fn new(client: ClientWithMiddleware, token_base_url: Url) -> Self {
        Self {
            client,
            token_base_url,
            token: Arc::new(Mutex::new(PendingOrFetched::Fetched(None))),
        }
    }

    pub async fn get_token(&self) -> Result<Arc<Token>, GatewayError> {
        let sender_or_receiver = {
            let mut token = self.token.lock();
            match &*token {
                PendingOrFetched::Fetched(Some(token)) if token.is_fresh() => {
                    // The token is still fresh.
                    return Ok(token.clone());
                }
                PendingOrFetched::Fetched(_) => {
                    let (sender, _) = tokio::sync::broadcast::channel(1);
                    let sender = Arc::new(sender);
                    *token = PendingOrFetched::Pending(Arc::downgrade(&sender));

                    Either::Left(sender)
                }
                PendingOrFetched::Pending(sender) => {
                    let sender = sender.upgrade();
                    if let Some(sender) = sender {
                        Either::Right(sender.subscribe())
                    } else {
                        let (sender, _) = tokio::sync::broadcast::channel(1);
                        let sender = Arc::new(sender);
                        *token = PendingOrFetched::Pending(Arc::downgrade(&sender));
                        Either::Left(sender)
                    }
                }
            }
        };

        let sender = match sender_or_receiver {
            Either::Left(sender) => sender,
            Either::Right(mut receiver) => {
                return match receiver.recv().await {
                    Ok(Some(token)) => Ok(token),
                    _ => {
                        // If this happens the sender was dropped.
                        Err(GatewayError::IoError(
                            "a coalesced request for a token failed".to_string(),
                            std::io::ErrorKind::Other.into(),
                        ))
                    }
                };
            }
        };

        let token_url = self
            .token_base_url
            .join("token")
            .expect("invalid token url");
        tracing::debug!("fetching token from {}", &token_url);

        // Fetch the token
        let response = self
            .client
            .get(token_url)
            .header(CACHE_CONTROL, HeaderValue::from_static("max-age=0"))
            .send()
            .await
            .and_then(|r| r.error_for_status().map_err(Into::into))
            .map_err(FetchRepoDataError::from)
            .map_err(GatewayError::from)?;

        let token = response
            .json::<Token>()
            .await
            .map_err(FetchRepoDataError::from)
            .map_err(GatewayError::from)
            .map(Arc::new)?;

        // Reacquire the token
        let mut token_lock = self.token.lock();
        *token_lock = PendingOrFetched::Fetched(Some(token.clone()));

        // Publish the change
        let _ = sender.send(Some(token.clone()));

        Ok(token)
    }
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

        let channel_base_url =
            Url::parse(&format!("https://fast.prefiks.dev/conda-forge/{subdir}/")).unwrap();
        let token_client = TokenClient::new(client.clone(), channel_base_url.clone());

        let sharded_repodata =
            fetch_index(client.clone(), &channel_base_url, &token_client, &cache_dir).await?;

        // Determine the cache directory and make sure it exists.
        let cache_dir = cache_dir.join("shards-v1");
        tokio::fs::create_dir_all(&cache_dir)
            .await
            .map_err(FetchRepoDataError::IoError)?;

        Ok(Self {
            channel,
            client,
            channel_base_url,
            token_client,
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

        // Get the token
        let token = self.token_client.get_token().await?;

        // Download the shard
        let shard_url = token
            .shard_base_url
            .as_ref()
            .unwrap_or(&self.channel_base_url)
            .join(&format!("shards/{shard:x}.msgpack.zst"))
            .expect("invalid shard url");

        let mut shard_request = self
            .client
            .get(shard_url.clone())
            .header(CACHE_CONTROL, HeaderValue::from_static("no-store"))
            .build()
            .expect("failed to build shard request");
        token.add_to_headers(shard_request.headers_mut());

        let shard_response = self
            .client
            .execute(shard_request)
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

/// The token endpoint response.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Token {
    token: Option<String>,
    issued_at: Option<DateTime<Utc>>,
    expires_in: Option<u64>,
    shard_base_url: Option<Url>,
}

impl Token {
    /// Returns true if the token is still considered to be valid.
    pub fn is_fresh(&self) -> bool {
        if let (Some(issued_at), Some(expires_in)) = (&self.issued_at, self.expires_in) {
            let now = Utc::now();
            if issued_at.add(TimeDelta::seconds(expires_in as i64)) > now {
                return false;
            }
        }
        true
    }

    /// Add the token to the headers if its available
    pub fn add_to_headers(&self, headers: &mut http::header::HeaderMap) {
        if let Some(token) = &self.token {
            headers.insert(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
        }
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
