//! Provides methods to download repodata from a given channel URL but does not
//! perform any form of caching.

use bytes::Bytes;
use futures::TryStreamExt;
use rattler_networking::retry_policies::default_retry_policy;
use rattler_redaction::Redact;
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Request, Response, StatusCode,
};
use retry_policies::{RetryDecision, RetryPolicy};
use std::{io::ErrorKind, sync::Arc};
use tokio::io::AsyncReadExt;
use tokio_util::io::StreamReader;
use tracing::{instrument, Level};
use url::Url;

#[cfg(target_arch = "wasm32")]
use wasmtimer::std::SystemTime;

#[cfg(not(target_arch = "wasm32"))]
use std::time::SystemTime;

use crate::{
    fetch::{FetchRepoDataError, RepoDataNotFoundError, Variant},
    reporter::ResponseReporterExt,
    utils::{AsyncEncoding, Encoding},
    Reporter,
};

/// Additional knobs that allow you to tweak the behavior of
/// [`fetch_repo_data`].
#[derive(Clone)]
pub struct FetchRepoDataOptions {
    /// Determines which variant to download. See [`Variant`] for more
    /// information.
    pub variant: Variant,

    /// When enabled, the zstd variant will be used if available
    pub zstd_enabled: bool,

    /// When enabled, the bz2 variant will be used if available
    pub bz2_enabled: bool,

    /// Retry policy to use when streaming the response is interrupted. If this
    /// is `None` the default retry policy is used.
    pub retry_policy: Option<Arc<dyn RetryPolicy + Send + Sync>>,
}

impl Default for FetchRepoDataOptions {
    fn default() -> Self {
        Self {
            variant: Variant::default(),
            zstd_enabled: true,
            bz2_enabled: true,
            retry_policy: None,
        }
    }
}

#[derive(Debug, Copy, Clone)]
enum Compression {
    Zst,
    Bz2,
    None,
}

impl From<Compression> for Encoding {
    fn from(value: Compression) -> Self {
        match value {
            Compression::Zst => Encoding::Zst,
            Compression::Bz2 => Encoding::Bz2,
            Compression::None => Encoding::Passthrough,
        }
    }
}

/// Try to execute a request for a certain kind of repodata with a given
/// compression.
async fn execute_request(
    subdir_url: Url,
    variant: Variant,
    method: Compression,
    client: reqwest_middleware::ClientWithMiddleware,
) -> Result<(Request, Response, SystemTime), reqwest_middleware::Error> {
    // Determine the URL of the repodata file based on the compression and the
    // variant
    let file_name = variant.file_name();
    let repo_data_url = match method {
        Compression::Zst => subdir_url.join(&format!("{file_name}.zst")),
        Compression::Bz2 => subdir_url.join(&format!("{file_name}.bz2")),
        Compression::None => subdir_url.join(variant.file_name()),
    }
    .expect("must be valid url at this point");

    // Construct a request
    let request_builder = client.get(repo_data_url.clone());

    let mut headers = HeaderMap::default();

    // We can handle g-zip encoding which is often used. We could also set this
    // option on the client, but that will disable all download progress
    // messages by `reqwest` because the gzipped data is decoded on the fly and
    // the size of the decompressed body is unknown. However, we don't really
    // care about the decompressed size but rather we'd like to know the number
    // of raw bytes that are actually downloaded.
    //
    // To do this we manually set the request header to accept gzip encoding and we
    // use the [`AsyncEncoding`] trait to perform the decoding on the fly.
    headers.insert(
        reqwest::header::ACCEPT_ENCODING,
        HeaderValue::from_static("gzip"),
    );

    let request = request_builder
        .headers(headers)
        .build()
        .expect("must have a valid request at this point");

    let request_start_time = SystemTime::now();
    let response = client
        .execute(request.try_clone().expect("cloning request cannot fail"))
        .await?;

    Ok((request, response, request_start_time))
}

/// Execute a request with the best compression method available. Returns the
/// request and the response.
async fn execute_with_best_compression(
    subdir_url: &Url,
    options: &FetchRepoDataOptions,
    client: reqwest_middleware::ClientWithMiddleware,
) -> Result<(Request, Response, SystemTime, Compression), FetchRepoDataError> {
    // Try with supported compression methods.
    for compression in [
        options.zstd_enabled.then_some(Compression::Zst),
        options.bz2_enabled.then_some(Compression::Bz2),
    ]
    .into_iter()
    .flatten()
    {
        let (request, response, request_time) = execute_request(
            subdir_url.clone(),
            options.variant,
            compression,
            client.clone(),
        )
        .await?;
        if response.status() == StatusCode::NOT_FOUND {
            continue;
        }
        return Ok((
            request,
            response.error_for_status()?,
            request_time,
            compression,
        ));
    }

    // If none of the compressed variants are available, try the uncompressed one.
    let (request, response, request_time) = execute_request(
        subdir_url.clone(),
        options.variant,
        Compression::None,
        client.clone(),
    )
    .await?;
    if response.status() == StatusCode::NOT_FOUND {
        Err(FetchRepoDataError::NotFound(
            RepoDataNotFoundError::HttpError(response.error_for_status().unwrap_err()),
        ))
    } else {
        Ok((
            request,
            response.error_for_status()?,
            request_time,
            Compression::None,
        ))
    }
}

/// Fetch the repodata.json file for the given subdirectory.
///
/// The successful result of this function returns the bytes of the uncompressed
/// repodata.json.
///
/// This method implements several different methods to download the
/// repodata.json file from the remote:
///
/// * If a `repodata.json.zst` file is available in the same directory that file
///   is downloaded and decompressed.
/// * If a `repodata.json.bz2` file is available in the same directory that file
///   is downloaded and decompressed.
/// * Otherwise the regular `repodata.json` file is downloaded.
///
/// Nothing is cached by this function.

#[instrument(err(level = Level::INFO), skip_all, fields(subdir_url))]
pub async fn fetch_repo_data(
    subdir_url: Url,
    client: reqwest_middleware::ClientWithMiddleware,
    options: FetchRepoDataOptions,
    reporter: Option<Arc<dyn Reporter>>,
) -> Result<Bytes, FetchRepoDataError> {
    // Try to download the repodata with the best compression method available.
    let (request, response, request_time, compression) =
        execute_with_best_compression(&subdir_url, &options, client.clone()).await?;

    // Notify that a request has started
    let repo_data_url = request.url().clone();
    let download_reporter = reporter
        .as_deref()
        .map(|r| (r, r.on_download_start(&repo_data_url)));

    // Construct a retry behavior
    let default_retry_behavior = default_retry_policy();
    let retry_behavior = options
        .retry_policy
        .as_deref()
        .unwrap_or(&default_retry_behavior);

    let mut retry_count = 0;
    let mut response = Some(response);
    let (bytes, response_url) = loop {
        // Either execute the request and get the response or use the response from a
        // previous execution.
        let (response, request_start_time) = match response.take() {
            None => {
                let start_time = SystemTime::now();
                let response = client
                    .execute(request.try_clone().expect("cloning request cannot fail"))
                    .await?
                    .error_for_status()?;
                (response, start_time)
            }
            Some(response) => (response, request_time),
        };

        // Stream the response the bytes and decode them on the fly.
        let (download_url, stream_error) =
            match stream_response_body(response, compression, download_reporter).await {
                Ok(bytes) => break (bytes, repo_data_url),
                Err(FetchRepoDataError::FailedToDownload(url, err)) => (url, err),
                Err(e) => {
                    return Err(e);
                }
            };

        let since_epoch = request_start_time.elapsed().unwrap();

        // Check if we can retry
        let execute_after = match retry_behavior.should_retry(
            std::time::SystemTime::UNIX_EPOCH
                .checked_add(since_epoch)
                .unwrap(),
            retry_count,
        ) {
            RetryDecision::Retry { execute_after } => SystemTime::UNIX_EPOCH
                .checked_add(execute_after.elapsed().unwrap())
                .unwrap(),
            RetryDecision::DoNotRetry => {
                return Err(FetchRepoDataError::FailedToDownload(
                    download_url,
                    stream_error,
                ))
            }
        };

        // Determine how long to sleep for
        let sleep_duration = execute_after
            .duration_since(SystemTime::now())
            .unwrap_or_default();

        #[cfg(not(target_arch = "wasm32"))]
        tokio::time::sleep(sleep_duration).await;
        #[cfg(target_arch = "wasm32")]
        wasmtimer::tokio::sleep(sleep_duration).await;

        retry_count += 1;
    };

    if let Some((reporter, index)) = download_reporter {
        reporter.on_download_complete(&response_url, index);
    }

    Ok(bytes)
}

async fn stream_response_body(
    response: Response,
    compression: Compression,
    reporter: Option<(&dyn Reporter, usize)>,
) -> Result<Bytes, FetchRepoDataError> {
    let response_url = response.url().clone().redact();
    let encoding = Encoding::from(&response);

    let mut total_bytes = 0;
    let bytes_stream = response
        .byte_stream_with_progress(reporter)
        .inspect_ok(|bytes| {
            total_bytes += bytes.len();
        })
        .map_err(|e| std::io::Error::new(ErrorKind::Interrupted, e));

    // Create a new stream from the byte stream that decodes the bytes using the
    // transfer encoding on the fly.
    let decoded_byte_stream = StreamReader::new(bytes_stream).decode(encoding);

    // Create yet another stream that decodes the bytes yet again but this time
    // using the content encoding.
    let mut decoded_repo_data_json_bytes =
        tokio::io::BufReader::new(decoded_byte_stream).decode(compression.into());

    let mut bytes = Vec::new();
    decoded_repo_data_json_bytes
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| FetchRepoDataError::FailedToDownload(response_url, e))?;

    Ok(Bytes::from(bytes))
}
