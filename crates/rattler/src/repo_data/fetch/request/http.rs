//! Defines the [`fetch_repodata`] function which downloads and caches repodata requests over http.

use std::{
    fs::File,
    io::{self, BufReader, BufWriter, ErrorKind, Read, Write},
    path::Path,
};

use bytes::Bytes;
use futures::{Stream, TryFutureExt, TryStreamExt};
use reqwest::header::{
    HeaderMap, HeaderValue, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use reqwest::StatusCode;
use serde_with::{serde_as, DisplayFromStr};
use tempfile::NamedTempFile;
use tokio::io::AsyncReadExt;
use tokio_util::io::StreamReader;
use url::Url;

use crate::{
    repo_data::fetch::{DoneState, DownloadingState, RepoDataRequestState, RequestRepoDataError},
    utils::{url_to_cache_filename, AsyncEncoding, Encoding},
};
use rattler_conda_types::RepoData;

/// Information stored along the repodata json that defines some caching properties.
#[serde_as]
#[derive(serde::Serialize, serde::Deserialize, Debug, Eq, PartialEq, Clone)]
struct RepoDataMetadata {
    #[serde(rename = "_url")]
    #[serde_as(as = "DisplayFromStr")]
    url: Url,

    #[serde(rename = "_etag")]
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,

    #[serde(rename = "_last_modified")]
    #[serde(skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
}

/// Downloads the repodata from the specified Url. The Url must point to a "repodata.json" file.
///
/// Requests can be cached by specifying a `cache_dir`. If the cache_dir is specified it will be
/// searched for a valid cache entry. If there is a cache hit, information from it will be send to
/// the remote. Only when there is new information on the server the repodata is downloaded,
/// otherwise it is fetched from the local cache. If no `cache_dir` is specified the repodata is
/// always completely downloaded from the server.
///
/// The `listener` parameter allows following the progress of the request through its various
/// stages. See [`RepoDataRequestState`] for the various stages a request can go through. As a
/// downloading repodata can take several seconds the `listener` can be used to show some visual
/// feedback to the user.
pub async fn fetch_repodata(
    url: Url,
    client: reqwest::Client,
    cache_dir: Option<&Path>,
    listener: &mut impl FnMut(RepoDataRequestState),
) -> Result<(RepoData, DoneState), RequestRepoDataError> {
    // If a cache directory has been set for this this request try looking up a cached entry and
    // read the metadata from it. If any error occurs during the loading of the cache we simply
    // ignore it and continue without a cache.
    let (metadata, cache_data) = if let Some(cache_dir) = cache_dir {
        let cache_path = cache_dir
            .join(url_to_cache_filename(&url))
            .with_extension("json");
        match read_cache_file(&cache_path) {
            Ok((metadata, cache_data)) => (Some(metadata), Some(cache_data)),
            _ => (None, None),
        }
    } else {
        (None, None)
    };

    let mut headers = HeaderMap::default();

    // We can handle g-zip encoding which is often used. We could also set this option on the
    // client, but that will disable all download progress messages by `reqwest` because the
    // gzipped data is decoded on the fly and the size of the decompressed body is unknown.
    // However, we don't really care about the decompressed size but rather we'd like to know
    // the number of raw bytes that are actually downloaded.
    //
    // To do this we manually set the request header to accept gzip encoding and we use the
    // [`AsyncEncoding`] trait to perform the decoding on the fly.
    headers.insert(
        reqwest::header::ACCEPT_ENCODING,
        HeaderValue::from_static("gzip"),
    );

    // Add headers that provide our caching behavior. We record the ETag that was previously send by
    // the server as well as the last-modified header.
    if let Some(metadata) = metadata {
        if metadata.url == url {
            if let Some(etag) = metadata
                .etag
                .and_then(|etag| HeaderValue::from_str(&etag).ok())
            {
                headers.insert(IF_NONE_MATCH, etag);
            }
            if let Some(last_modified) = metadata
                .last_modified
                .and_then(|etag| HeaderValue::from_str(&etag).ok())
            {
                headers.insert(IF_MODIFIED_SINCE, last_modified);
            }
        }
    }

    // Construct a request to the server and dispatch it.
    let response = client
        .get(url.clone())
        .headers(headers)
        .send()
        .await?
        .error_for_status()?;

    // If the server replied with a NOT_MODIFIED status it means that the ETag or the last modified
    // date we send along actually matches whats already on the server or the contents didnt change
    // since the last time we queried the data. This means we can use the cached data.
    if response.status() == StatusCode::NOT_MODIFIED {
        // Now that we have all the data in memory we can deserialize the content using `serde`.
        // Since repodata information can be quite huge we run the deserialization in a separate
        // background task to ensure we don't block the current thread.
        listener(RepoDataRequestState::Deserializing);
        let repodata = tokio::task::spawn_blocking(move || {
            serde_json::from_slice(cache_data.unwrap().as_slice())
        })
        .await??;
        return Ok((repodata, DoneState { cache_miss: false }));
    }

    // Determine the length of the response in bytes and notify the listener that a download is
    // starting. The response may be compressed. Decompression happens below.
    let content_size = response.content_length().map(|len| len as usize);
    listener(
        DownloadingState {
            bytes: 0,
            total: content_size,
        }
        .into(),
    );

    // Get the ETag from the response (if any). This can be used to cache the result during a next
    // request.
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|header| header.to_str().ok())
        .map(ToOwned::to_owned);

    // Get the last modified time. This can also be used to cache the result during a next request.
    let last_modified = response
        .headers()
        .get(LAST_MODIFIED)
        .and_then(|header| header.to_str().ok())
        .map(ToOwned::to_owned);

    // Get the request as a stream of bytes. Download progress is added through the
    // [`add_download_progress_listener`] function, and decompression happens through the
    // [`AsyncEncoding::decode`] function.
    let encoding = Encoding::from(&response);
    let bytes_stream =
        add_download_progress_listener(response.bytes_stream(), listener, content_size);
    let mut decoded_byte_stream =
        StreamReader::new(bytes_stream.map_err(|e| io::Error::new(ErrorKind::Other, e)))
            .decode(encoding);

    // The above code didn't actually perform any downloading. This code allocates memory to read
    // the downloaded information to. The [`AsyncReadExt::read_to_end`] function than actually
    // downloads all the bytes. The bytes are decompressed on the fly.
    //
    // By now, we know that the data we read from cache is out of date, so we can reuse the memory
    // allocated for it, although we do clear it out first. If we dont have any pre-allocated cache
    // data, we allocate a new block of memory.
    //
    // We don't know what the decompressed size of the bytes will be but a good guess is simply the
    // size of the response body. If we don't know the size of the body we start with 1MB.
    let mut data = cache_data
        .map(|mut data| {
            data.clear();
            data
        })
        .unwrap_or_else(|| Vec::with_capacity(content_size.unwrap_or(1_073_741_824) as usize));
    decoded_byte_stream.read_to_end(&mut data).await?;
    let bytes = Bytes::from(data);

    // Explicitly drop the byte stream, this is required to ensure that we can safely use the
    // mutable listener that was captured by the download progress.
    drop(decoded_byte_stream);

    // If there is a cache directory write to the cache
    let caching_future = cache_repodata_response(
        cache_dir,
        RepoDataMetadata {
            url,
            etag,
            last_modified,
        },
        bytes.clone(),
    );

    // Now that we have all the data in memory we can deserialize the content using `serde`. Since
    // repodata information can be quite huge we run the deserialization in a separate background
    // task to ensure we don't block the current thread.
    listener(RepoDataRequestState::Deserializing);
    let deserializing_future = tokio::task::spawn_blocking(move || serde_json::from_slice(&bytes))
        .map_err(RequestRepoDataError::from)
        .and_then(|serde_result| async { serde_result.map_err(RequestRepoDataError::from) });

    // Await the result of caching and deserializing. This either returns immediately if any error
    // occurs or until both futures complete successfully.
    let (_, repodata) = tokio::try_join!(caching_future, deserializing_future)?;

    // If we get here, we have successfully downloaded (and potentially cached) the complete
    // repodata from the server.
    Ok((repodata, DoneState { cache_miss: true }))
}

/// Called to asynchronously cache the response from a HTTP request to the specified cache
/// directory. If the cache directory is `None` nothing happens.
async fn cache_repodata_response(
    cache_dir: Option<&Path>,
    metadata: RepoDataMetadata,
    bytes: Bytes,
) -> Result<(), RequestRepoDataError> {
    // Early out if the cache directory is empty
    let cache_dir = if let Some(cache_dir) = cache_dir {
        cache_dir.to_owned()
    } else {
        return Ok(());
    };

    // File system operations can be blocking which is why we do it on a separate thread through the
    // call to `spawn_blocking`. This ensures that any blocking operations are not run on the main
    // task.
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&cache_dir)?;
        let cache_path = cache_dir
            .join(url_to_cache_filename(&metadata.url))
            .with_extension("json");
        let cache_file = create_cache_file(metadata, &bytes)?;
        cache_file.persist(&cache_path)?;
        Ok(())
    })
    .await?
}

/// Writes the bytes encoded as JSON object to a file together with the specified metadata.
///
/// This function concatenates the metadata json and the `raw_bytes` together by removing the
/// trailing `}` of the metadata json, adding a `,` and removing the preceding `{` from the raw
/// bytes. If any of these characters cannot be located in the data the function panics.
///
/// On success a [`NamedTempFile`] is returned which contains the resulting json. Its up to the
/// caller to either persist this file. See [`NamedTempFile::persist`] for more information.
fn create_cache_file(metadata: RepoDataMetadata, raw_bytes: &[u8]) -> io::Result<NamedTempFile> {
    // Convert the metadata to json
    let metadata_json =
        serde_json::to_string(&metadata).expect("converting metadata to json shouldn't fail");

    // Open the cache file.
    let mut temp_file = NamedTempFile::new()?;
    let mut writer = BufWriter::new(temp_file.as_file_mut());

    // Strip the trailing closing '}' so we can append the rest of the json.
    let stripped_metadata_json = metadata_json
        .strip_suffix('}')
        .expect("expected metadata to end with a '}'");

    // Strip the preceding opening '{' from the raw data.
    let stripped_raw_bytes = raw_bytes
        .strip_prefix(b"{")
        .expect("expected the repodata to be preceded by an opening '{'");

    // Write the contents of the metadata, followed by the contents of the raw bytes.
    writer.write_all(stripped_metadata_json.as_bytes())?;
    writer.write_all(",".as_bytes())?;
    writer.write_all(stripped_raw_bytes)?;

    // Drop the writer so we can return the temp file
    drop(writer);

    Ok(temp_file)
}

/// Reads a cache file and return the contents of it as well as the metadata read from the bytes.
///
/// A repodata cache file contains the original json read from the remote as well as extra
/// information called metadata (see [`RepoDatametadata`]) which is injected after the data is
/// received from the remote. The metadata can be used to determine if the data stored in the cache
/// is actually current and doesnt need to be updated.
fn read_cache_file(cache_path: &Path) -> anyhow::Result<(RepoDataMetadata, Vec<u8>)> {
    // Read the contents of the entire cache file to memory
    let mut reader = BufReader::new(File::open(cache_path)?);
    let mut cache_data = Vec::new();
    reader.read_to_end(&mut cache_data)?;

    // Parse the metadata from the data
    let metadata: RepoDataMetadata = serde_json::from_slice(&cache_data)?;

    Ok((metadata, cache_data))
}

/// Modifies the input stream to emit download information to the specified listener on the fly.
fn add_download_progress_listener<'s, E>(
    stream: impl Stream<Item = Result<Bytes, E>> + 's,
    listener: &'s mut impl FnMut(RepoDataRequestState),
    content_length: Option<usize>,
) -> impl Stream<Item = Result<Bytes, E>> + 's {
    let mut bytes_downloaded = 0;
    stream.inspect_ok(move |bytes| {
        bytes_downloaded += bytes.len();
        listener(
            DownloadingState {
                bytes: bytes_downloaded,
                total: content_length,
            }
            .into(),
        );
    })
}

#[cfg(test)]
mod test {
    use std::fs::File;
    use std::io::BufReader;
    use std::str::FromStr;
    use tempfile::TempDir;
    use url::Url;

    use super::{create_cache_file, fetch_repodata, read_cache_file, RepoDataMetadata};
    use crate::repo_data::fetch::request::REPODATA_CHANNEL_PATH;
    use crate::utils::simple_channel_server::SimpleChannelServer;
    use rattler_conda_types::{Channel, ChannelConfig, Platform};
    use crate::get_test_data_dir;

    #[tokio::test]
    async fn test_fetch_http() {
        let channel_path = get_test_data_dir().join("channels/empty");

        let server = SimpleChannelServer::new(channel_path);
        let url = server.url().to_string();
        let channel = Channel::from_str(url, &ChannelConfig::default()).unwrap();

        let _result = fetch_repodata(
            channel
                .platform_url(Platform::NoArch)
                .join(REPODATA_CHANNEL_PATH)
                .unwrap(),
            reqwest::Client::default(),
            None,
            &mut |_| {},
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_http_fetch_cache() {
        let channel_path = get_test_data_dir().join("channels/empty");

        let server = SimpleChannelServer::new(channel_path);
        let url = server.url().to_string();
        let channel = Channel::from_str(url, &ChannelConfig::default()).unwrap();

        // Create a temporary directory to store the cache in
        let cache_dir = TempDir::new().unwrap();

        // Fetch the repodata from the server
        let (repodata, done_state) = fetch_repodata(
            channel
                .platform_url(Platform::NoArch)
                .join(REPODATA_CHANNEL_PATH)
                .unwrap(),
            reqwest::Client::default(),
            Some(cache_dir.path()),
            &mut |_| {},
        )
        .await
        .unwrap();
        assert!(done_state.cache_miss);

        // Fetch the repodata again, and check that the result has been cached
        let (repodata_with_cache, cached_done_state) = fetch_repodata(
            channel
                .platform_url(Platform::NoArch)
                .join(REPODATA_CHANNEL_PATH)
                .unwrap(),
            reqwest::Client::default(),
            Some(cache_dir.path()),
            &mut |_| {},
        )
        .await
        .unwrap();

        assert!(!cached_done_state.cache_miss);
        assert_eq!(repodata, repodata_with_cache);
    }

    #[test]
    fn test_cache_in_cache_out() {
        #[derive(Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
        struct Foo {
            data: String,
        }

        let data = Foo {
            data: String::from("Hello, world!"),
        };
        let data_bytes = serde_json::to_vec(&data).unwrap();

        let metadata = RepoDataMetadata {
            url: Url::from_str("https://google.com").unwrap(),
            etag: Some(String::from("THIS IS NOT REALLY AN ETAG")),
            last_modified: Some(String::from("this is a last modified data or something")),
        };

        // Create a cached file
        let cache_file = create_cache_file(metadata.clone(), &data_bytes).unwrap();

        // The cache file still contains valid json
        let _: serde_json::Value =
            serde_json::from_reader(BufReader::new(File::open(cache_file.path()).unwrap()))
                .expect("cache file doesnt contain valid json");

        // Read the cached file again
        let (result_metadata, result_bytes) = read_cache_file(cache_file.path()).unwrap();

        // See if the data from the cache matches that what we wrote to it.
        assert_eq!(data, serde_json::from_slice::<Foo>(&result_bytes).unwrap());
        assert_eq!(metadata, result_metadata);
    }
}
