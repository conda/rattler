//! Download a single remote file as several byte ranges fetched in parallel.
//!
//! A single connection to an object store like S3 tops out well below the
//! bandwidth of the machine, so large packages download much faster when their
//! bytes are requested as concurrent ranges. The parts are yielded in order, so
//! callers can keep streaming the result into an extractor.

use std::{io::ErrorKind, ops::Range, time::Duration};

use bytes::{Bytes, BytesMut};
use futures_util::{
    StreamExt, TryStreamExt, future,
    stream::{self, BoxStream},
};
use http::{HeaderValue, StatusCode, header};
use rattler_digest::Sha256Hash;
use rattler_networking::mirror_middleware::Mirrored;
use reqwest::Response;
use reqwest_middleware::ClientWithMiddleware;
use tokio_util::task::AbortOnDropHandle;
use url::Url;

use super::{
    DownloadOptions,
    tokio::{Download, get, to_io_error},
};

/// A response that cannot be stitched together with other ranges.
#[derive(Debug, thiserror::Error)]
enum RangeError {
    #[error("expected a 206 response for bytes {start}-{end} but got {status}")]
    NotPartial {
        start: u64,
        end: u64,
        status: StatusCode,
    },
    #[error("expected Content-Range bytes {start}-{end}/{total} but got {got:?}")]
    WrongContentRange {
        start: u64,
        end: u64,
        total: u64,
        got: Option<String>,
    },
    #[error("expected {expected} bytes for range {start}-{end} but got {got}")]
    WrongLength {
        start: u64,
        end: u64,
        expected: u64,
        got: u64,
    },
    #[error("the object changed while it was downloading (expected ETag {etag})")]
    Changed { etag: String },
}

/// How many times a part is requested before a dropped connection fails it.
const PART_ATTEMPTS: u32 = 3;

impl From<RangeError> for std::io::Error {
    fn from(err: RangeError) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, err)
    }
}

/// Download `url`, fetching it in parallel ranges when the server supports it.
///
/// The first request asks for the first part only. A server that ignores the
/// range, or a file that fits in one part, is streamed from that response as
/// is. Otherwise the remaining parts are requested concurrently.
pub(crate) async fn download(
    client: ClientWithMiddleware,
    url: Url,
    expected_sha256: Option<Sha256Hash>,
    options: DownloadOptions,
) -> reqwest_middleware::Result<Download> {
    let first = get(&client, url.clone(), expected_sha256)
        .header(header::RANGE, range_header(&(0..options.part_size)))
        .send()
        .await?;

    // An empty file cannot satisfy any range.
    if first.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        return whole(get(&client, url, expected_sha256).send().await?);
    }
    let first = first.error_for_status()?;
    if first.status() != StatusCode::PARTIAL_CONTENT {
        return whole(first);
    }
    // A mirror is chosen per request, so the parts could come from different
    // mirrors, or each repeat an OCI registry lookup.
    if first.extensions().get::<Mirrored>().is_some() {
        drop(first);
        return whole(get(&client, url, expected_sha256).send().await?);
    }

    let Some((first_range, total)) = content_range(&first)
        .filter(|(range, total)| range.start == 0 && range.end > 0 && range.end <= *total)
    else {
        // Without the size of the file the remaining parts can't be planned.
        tracing::debug!(
            "unusable Content-Range from {}, downloading it whole",
            first.url()
        );
        return whole(get(&client, url, expected_sha256).send().await?);
    };
    if first_range.end == total {
        return Ok(Download {
            total_bytes: Some(total),
            body: response_body(first),
        });
    }

    // A server may return less than asked for, and the same for every range.
    let part_size = first_range.end.min(options.part_size);
    let parts_per_download = options.parts;
    let parts = Parts {
        client,
        url,
        expected_sha256,
        total,
        etag: s3_etag(&first),
        options,
    };
    let mut first = Some(first);
    let tasks = part_ranges(first_range.end, total, part_size).map(move |range| {
        let parts = parts.clone();
        // Dropping the stream stops the parts that are still downloading.
        let task = AbortOnDropHandle::new(match first.take() {
            Some(first) => tokio::spawn(async move {
                match read_part(first, range.clone(), parts.total).await {
                    Err(err) if err.kind() == ErrorKind::Interrupted => parts.fetch(range).await,
                    result => result,
                }
            }),
            None => tokio::spawn(async move { parts.fetch(range).await }),
        });
        async move {
            match task.await {
                Ok(result) => result,
                Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
                Err(err) => Err(std::io::Error::other(err)),
            }
        }
    });

    Ok(Download {
        total_bytes: Some(total),
        body: stream::iter(tasks)
            .buffered(parts_per_download)
            // A reader may retry an `Interrupted` read, which must not skip
            // the part that failed.
            .scan(false, |failed, part| {
                let ended = std::mem::replace(failed, part.is_err());
                future::ready((!ended).then_some(part))
            })
            .boxed(),
    })
}

/// The ranges of a `total` byte file whose first part ends at `first_end`.
fn part_ranges(first_end: u64, total: u64, part_size: u64) -> impl Iterator<Item = Range<u64>> {
    let rest = (first_end..total)
        .step_by(usize::try_from(part_size).unwrap_or(usize::MAX))
        .map(move |start| start..(start.saturating_add(part_size)).min(total));
    std::iter::once(0..first_end).chain(rest)
}

fn whole(response: Response) -> reqwest_middleware::Result<Download> {
    let response = response.error_for_status()?;
    Ok(Download {
        total_bytes: response.content_length(),
        body: response_body(response),
    })
}

/// The entity tag every part must carry, when the server is S3.
///
/// S3 entity tags identify an object's content, so a part with another tag is
/// from an object overwritten mid-download. Other servers may compute tags per
/// replica, so only S3's are compared.
fn s3_etag(response: &Response) -> Option<HeaderValue> {
    let headers = response.headers();
    let is_s3 = headers.contains_key("x-amz-request-id")
        || headers
            .get(header::SERVER)
            .is_some_and(|server| server == "AmazonS3");
    headers
        .get(header::ETAG)
        .filter(|etag| is_s3 && !etag.as_bytes().starts_with(b"W/"))
        .cloned()
}

/// What the requests for the parts after the first share.
#[derive(Clone)]
struct Parts {
    client: ClientWithMiddleware,
    url: Url,
    expected_sha256: Option<Sha256Hash>,
    total: u64,
    etag: Option<HeaderValue>,
    options: DownloadOptions,
}

impl Parts {
    /// Fetch `range`, requesting it again when the connection drops.
    async fn fetch(&self, range: Range<u64>) -> std::io::Result<Bytes> {
        let mut attempt = 1;
        loop {
            match self.fetch_once(range.clone()).await {
                Err(err) if err.kind() == ErrorKind::Interrupted && attempt < PART_ATTEMPTS => {
                    tracing::debug!("retrying bytes {range:?} of {}: {err}", self.url);
                    tokio::time::sleep(Duration::from_millis(100 << attempt)).await;
                    attempt += 1;
                }
                result => return result,
            }
        }
    }

    async fn fetch_once(&self, range: Range<u64>) -> std::io::Result<Bytes> {
        // Held only while the part downloads, so a slow reader of one package
        // can't stall the others.
        let _permit = self
            .options
            .parts_in_flight
            .acquire()
            .await
            .map_err(std::io::Error::other)?;
        let mut request = get(&self.client, self.url.clone(), self.expected_sha256)
            .header(header::RANGE, range_header(&range));
        if let Some(etag) = &self.etag {
            request = request.header(header::IF_MATCH, etag.clone());
        }
        let response = request.send().await.map_err(|err| match err {
            reqwest_middleware::Error::Reqwest(err) if err.is_connect() || err.is_timeout() => {
                std::io::Error::new(ErrorKind::Interrupted, err)
            }
            reqwest_middleware::Error::Reqwest(err) => to_io_error(err),
            reqwest_middleware::Error::Middleware(err) => std::io::Error::other(err),
        })?;
        if let Some(etag) = &self.etag {
            // An error response carries no ETag, so it is reported as itself.
            let changed = response.status() == StatusCode::PRECONDITION_FAILED
                || (response.status().is_success()
                    && response.headers().get(header::ETAG) != Some(etag));
            if changed {
                return Err(RangeError::Changed {
                    etag: String::from_utf8_lossy(etag.as_bytes()).into_owned(),
                }
                .into());
            }
        }
        let response = response.error_for_status().map_err(to_io_error)?;
        read_part(response, range, self.total).await
    }
}

/// Read the body of a response to a range request after checking it holds
/// exactly the bytes asked for.
async fn read_part(
    mut response: Response,
    range: Range<u64>,
    total: u64,
) -> std::io::Result<Bytes> {
    let (start, end) = (range.start, range.end - 1);
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(RangeError::NotPartial {
            start,
            end,
            status: response.status(),
        }
        .into());
    }
    if content_range(&response) != Some((range.clone(), total)) {
        return Err(RangeError::WrongContentRange {
            start,
            end,
            total,
            got: response
                .headers()
                .get(header::CONTENT_RANGE)
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned()),
        }
        .into());
    }
    let expected = range.end - range.start;
    let wrong_length = |got| RangeError::WrongLength {
        start,
        end,
        expected,
        got,
    };
    // Stop reading a body as soon as it is longer than the range.
    let mut bytes = BytesMut::new();
    while let Some(chunk) = response.chunk().await.map_err(dropped)? {
        let got = (bytes.len() + chunk.len()) as u64;
        if got > expected {
            return Err(wrong_length(got).into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 != expected {
        return Err(wrong_length(bytes.len() as u64).into());
    }
    Ok(bytes.freeze())
}

/// An error reading a part's body, which reqwest reports as a decode error
/// when the connection drops.
fn dropped(err: reqwest::Error) -> std::io::Error {
    if err.is_body() || err.is_decode() || err.is_timeout() {
        std::io::Error::new(ErrorKind::Interrupted, err)
    } else {
        to_io_error(err)
    }
}

fn range_header(range: &Range<u64>) -> String {
    format!("bytes={}-{}", range.start, range.end - 1)
}

/// The byte range, as a half-open range, and total size in `Content-Range`.
fn content_range(response: &Response) -> Option<(Range<u64>, u64)> {
    let value = response
        .headers()
        .get(header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let (start, end, total) = (
        start.parse().ok()?,
        end.parse::<u64>().ok()?,
        total.parse().ok()?,
    );
    (start <= end).then(|| (start..end + 1, total))
}

fn response_body(response: Response) -> BoxStream<'static, std::io::Result<Bytes>> {
    response.bytes_stream().map_err(to_io_error).boxed()
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::Ordering,
        time::Duration,
    };

    use futures_util::{StreamExt, TryStreamExt};
    use rattler_digest::Sha256;
    use tokio_util::io::StreamReader;

    use super::*;
    use crate::reqwest::test_server;

    const PART_SIZE: u64 = 256;
    const PARTS_IN_FLIGHT: usize = 3;

    fn config() -> DownloadOptions {
        DownloadOptions::new(PART_SIZE, PARTS_IN_FLIGHT)
    }

    fn test_file(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber")
            .join(name)
    }

    fn conda_file() -> PathBuf {
        test_file("clobber-fd-1-0.1.0-h4616a5c_0.conda")
    }

    fn parts_of(path: &Path) -> usize {
        std::fs::metadata(path).unwrap().len().div_ceil(PART_SIZE) as usize
    }

    async fn download_bytes(url: Url, config: DownloadOptions) -> std::io::Result<Vec<u8>> {
        let download = download(
            ClientWithMiddleware::from(reqwest::Client::new()),
            url,
            None,
            config,
        )
        .await
        .unwrap();
        let parts: Vec<Bytes> = download.body.try_collect().await?;
        Ok(parts.concat())
    }

    #[tokio::test]
    async fn parts_finishing_out_of_order_are_yielded_in_order() {
        // The second part is held back until the parts after it have finished.
        let (url, observed) = test_server::serve_file_observed(
            conda_file(),
            Duration::from_millis(10),
            PART_SIZE,
            Duration::from_millis(300),
        )
        .await;

        let bytes = download_bytes(url, config()).await.unwrap();

        assert_eq!(bytes, std::fs::read(conda_file()).unwrap());
        let finished = observed.finished.lock().unwrap().clone();
        assert_eq!(finished.len(), parts_of(&conda_file()));
        let position = |start| finished.iter().position(|s| *s == start).unwrap();
        assert!(
            position(2 * PART_SIZE) < position(PART_SIZE),
            "the third part finished before the second: {finished:?}"
        );
    }

    #[tokio::test]
    async fn requests_at_most_the_configured_parts_at_once() {
        let (url, observed) = test_server::serve_file_observed(
            conda_file(),
            Duration::from_millis(50),
            0,
            Duration::from_millis(50),
        )
        .await;

        download_bytes(url, config()).await.unwrap();

        let max_in_flight = observed.max_in_flight.load(Ordering::SeqCst);
        assert!(
            (2..=PARTS_IN_FLIGHT).contains(&max_in_flight),
            "{max_in_flight} requests were in flight at once"
        );
    }

    #[tokio::test]
    async fn downloads_sharing_options_share_the_limit() {
        let (url, observed) = test_server::serve_file_observed(
            conda_file(),
            Duration::from_millis(50),
            0,
            Duration::from_millis(50),
        )
        .await;
        let options = DownloadOptions::new(PART_SIZE, 2);

        let (a, b) = tokio::join!(
            download_bytes(url.clone(), options.clone()),
            download_bytes(url, options)
        );
        a.unwrap();
        b.unwrap();

        let max_parts = observed.max_later_parts_in_flight.load(Ordering::SeqCst);
        assert!(
            (1..=2).contains(&max_parts),
            "{max_parts} later parts were in flight at once"
        );
    }

    #[tokio::test]
    async fn packages_downloaded_in_parts_extract_with_the_right_hash() {
        for (name, is_conda) in [
            ("clobber-fd-1-0.1.0-h4616a5c_0.conda", true),
            ("clobber-1-0.1.0-h4616a5c_0.tar.bz2", false),
        ] {
            let path = test_file(name);
            let url = test_server::serve_file(&path).await;
            let download = download(
                ClientWithMiddleware::from(reqwest::Client::new()),
                url,
                None,
                config(),
            )
            .await
            .unwrap();
            let reader = StreamReader::new(download.body);
            let destination = tempfile::tempdir().unwrap();

            let result = if is_conda {
                crate::tokio::async_read::extract_conda(reader, destination.path()).await
            } else {
                crate::tokio::async_read::extract_tar_bz2(reader, destination.path()).await
            }
            .unwrap();

            assert_eq!(
                result.sha256,
                rattler_digest::compute_file_digest::<Sha256>(&path).unwrap(),
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn a_server_ignoring_ranges_is_read_from_the_first_response() {
        let url = test_server::serve_file_no_ranges(conda_file()).await;

        let bytes = download_bytes(url, config()).await.unwrap();

        assert_eq!(bytes, std::fs::read(conda_file()).unwrap());
    }

    #[tokio::test]
    async fn a_file_within_one_part_takes_one_request() {
        let (url, observed) =
            test_server::serve_file_observed(conda_file(), Duration::ZERO, 0, Duration::ZERO).await;
        let config = DownloadOptions::new(1024 * 1024, PARTS_IN_FLIGHT);

        let bytes = download_bytes(url, config).await.unwrap();

        assert_eq!(bytes, std::fs::read(conda_file()).unwrap());
        assert_eq!(*observed.finished.lock().unwrap(), vec![0]);
    }

    #[tokio::test]
    async fn an_empty_file_is_downloaded_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.conda");
        std::fs::write(&path, b"").unwrap();
        // No range of an empty file can be satisfied, so the server answers 416.
        let url = test_server::serve_file(&path).await;

        let bytes = download_bytes(url, config()).await.unwrap();

        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn a_part_answered_with_the_whole_file_is_an_error() {
        let url = test_server::serve_file_ranges_once(conda_file()).await;

        let err = download_bytes(url, config()).await.unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("expected a 206"), "{err}");
    }

    async fn download_overwritten(store: test_server::ObjectStore) -> std::io::Result<Vec<u8>> {
        let url = test_server::serve_file_overwritten(conda_file(), store).await;
        download_bytes(url, config()).await
    }

    fn assert_changed(result: std::io::Result<Vec<u8>>) {
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("changed while it was downloading"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn an_s3_object_overwritten_mid_download_fails() {
        assert_changed(
            download_overwritten(test_server::ObjectStore {
                s3: true,
                honors_if_match: true,
            })
            .await,
        );
    }

    #[tokio::test]
    async fn a_part_with_another_s3_etag_fails_when_if_match_is_ignored() {
        assert_changed(
            download_overwritten(test_server::ObjectStore {
                s3: true,
                honors_if_match: false,
            })
            .await,
        );
    }

    #[tokio::test]
    async fn etags_from_servers_other_than_s3_are_not_compared() {
        // Replicas behind a mirror may compute different ETags for one file.
        let bytes = download_overwritten(test_server::ObjectStore {
            s3: false,
            honors_if_match: false,
        })
        .await
        .unwrap();

        assert_eq!(bytes, std::fs::read(conda_file()).unwrap());
    }

    #[tokio::test]
    async fn an_unchanged_s3_object_downloads_in_parts() {
        let (url, matched) = test_server::serve_file_from_s3(conda_file()).await;

        let bytes = download_bytes(url, config()).await.unwrap();

        assert_eq!(bytes, std::fs::read(conda_file()).unwrap());
        // Every part after the first is pinned to the first part's ETag.
        assert_eq!(matched.load(Ordering::SeqCst), parts_of(&conda_file()) - 1);
    }

    #[tokio::test]
    async fn a_part_whose_connection_drops_is_requested_again() {
        for start in [0, PART_SIZE] {
            let (url, requests) = test_server::serve_file_breaking_part(
                conda_file(),
                start,
                test_server::Breakage::CutOnce,
            )
            .await;

            let bytes = download_bytes(url, config()).await.unwrap();

            assert_eq!(bytes, std::fs::read(conda_file()).unwrap(), "{start}");
            assert_eq!(requests.load(Ordering::SeqCst), 2, "{start}");
        }
    }

    #[tokio::test]
    async fn a_part_longer_than_its_range_is_an_error() {
        let (url, _) = test_server::serve_file_breaking_part(
            conda_file(),
            PART_SIZE,
            test_server::Breakage::Overlong,
        )
        .await;

        let err = download_bytes(url, config()).await.unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("expected 256 bytes"), "{err}");
    }

    #[tokio::test]
    async fn a_server_returning_shorter_ranges_is_read_in_its_part_size() {
        let url = test_server::serve_file_capping_ranges(conda_file(), PART_SIZE / 2).await;

        let bytes = download_bytes(url, config()).await.unwrap();

        assert_eq!(bytes, std::fs::read(conda_file()).unwrap());
    }

    #[tokio::test]
    async fn a_failed_part_ends_the_stream() {
        let url = test_server::serve_file_ranges_once(conda_file()).await;
        let mut body = download(
            ClientWithMiddleware::from(reqwest::Client::new()),
            url,
            None,
            config(),
        )
        .await
        .unwrap()
        .body;

        assert!(body.next().await.unwrap().is_ok());
        assert!(body.next().await.unwrap().is_err());
        assert!(body.next().await.is_none());
    }

    #[tokio::test]
    async fn a_mirrored_file_is_downloaded_whole() {
        let (file_url, observed) =
            test_server::serve_file_observed(conda_file(), Duration::ZERO, 0, Duration::ZERO).await;
        let channel: Url = "https://channel.invalid/".parse().unwrap();
        let mirror = rattler_networking::mirror_middleware::Mirror {
            url: file_url.join(".").unwrap(),
            no_zstd: false,
            no_bz2: false,
            max_failures: None,
        };
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(rattler_networking::MirrorMiddleware::from_map(
                [(channel.clone(), vec![mirror])].into(),
            ))
            .build();
        let url = channel
            .join(file_url.path_segments().unwrap().next_back().unwrap())
            .unwrap();

        let download = download(client, url, None, config()).await.unwrap();
        let parts: Vec<Bytes> = download.body.try_collect().await.unwrap();

        assert_eq!(parts.concat(), std::fs::read(conda_file()).unwrap());
        // Only the first request asked for a range.
        assert_eq!(*observed.finished.lock().unwrap(), vec![0]);
    }

    #[test]
    fn parts_cover_the_file_once() {
        let parts: Vec<_> = part_ranges(100, 1050, 400).collect();

        assert_eq!(parts, vec![0..100, 100..500, 500..900, 900..1050]);
    }
}
