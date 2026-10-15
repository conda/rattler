//! Download a single remote file as several byte ranges fetched in parallel.
//!
//! A single connection to an object store like S3 tops out well below the
//! bandwidth of the machine, so large packages download much faster when their
//! bytes are requested as concurrent ranges. The parts are yielded in order, so
//! callers can keep streaming the result into an extractor.

use std::ops::Range;

use bytes::Bytes;
use futures_util::{
    StreamExt, TryStreamExt,
    stream::{self, BoxStream},
};
use http::{StatusCode, header};
use rattler_digest::Sha256Hash;
use reqwest::Response;
use reqwest_middleware::ClientWithMiddleware;
use tokio_util::task::AbortOnDropHandle;
use url::Url;

use super::tokio::{Download, get, to_io_error};

/// How a file is split into parallel range requests.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RangedDownload {
    /// The number of bytes requested per range.
    pub part_size: u64,
    /// The number of parts requested at once. Parts that arrive ahead of an
    /// earlier one are held in memory and count against this limit, so at most
    /// `parts_in_flight * part_size` bytes are buffered per download.
    pub parts_in_flight: usize,
}

impl Default for RangedDownload {
    fn default() -> Self {
        Self {
            part_size: 8 * 1024 * 1024,
            parts_in_flight: 8,
        }
    }
}

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
}

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
    config: RangedDownload,
) -> reqwest_middleware::Result<Download> {
    let first = get(&client, url.clone(), expected_sha256)
        .header(header::RANGE, range_header(&(0..config.part_size)))
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

    let mut first = Some(first);
    let parts = part_ranges(first_range.end, total, config.part_size).map(move |range| {
        let task = match first.take() {
            Some(first) => tokio::spawn(read_part(first, range, total)),
            None => tokio::spawn(fetch_part(
                get(&client, url.clone(), expected_sha256),
                range,
                total,
            )),
        };
        // Dropping the stream stops the parts that are still downloading.
        async move {
            AbortOnDropHandle::new(task)
                .await
                .map_err(std::io::Error::other)?
        }
    });

    Ok(Download {
        total_bytes: Some(total),
        body: stream::iter(parts)
            .buffered(config.parts_in_flight.max(1))
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

async fn fetch_part(
    request: reqwest_middleware::RequestBuilder,
    range: Range<u64>,
    total: u64,
) -> std::io::Result<Bytes> {
    let response = request
        .header(header::RANGE, range_header(&range))
        .send()
        .await
        .map_err(|err| match err {
            reqwest_middleware::Error::Reqwest(err) => to_io_error(err),
            reqwest_middleware::Error::Middleware(err) => std::io::Error::other(err),
        })?
        .error_for_status()
        .map_err(to_io_error)?;
    read_part(response, range, total).await
}

/// Read the body of a response to a range request after checking it holds
/// exactly the bytes asked for.
async fn read_part(response: Response, range: Range<u64>, total: u64) -> std::io::Result<Bytes> {
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
    let bytes = response.bytes().await.map_err(to_io_error)?;
    let expected = range.end - range.start;
    if bytes.len() as u64 != expected {
        return Err(RangeError::WrongLength {
            start,
            end,
            expected,
            got: bytes.len() as u64,
        }
        .into());
    }
    Ok(bytes)
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

    use futures_util::TryStreamExt;
    use rattler_digest::Sha256;
    use tokio_util::io::StreamReader;

    use super::*;
    use crate::reqwest::test_server;

    const PART_SIZE: u64 = 256;
    const PARTS_IN_FLIGHT: usize = 3;

    fn config() -> RangedDownload {
        RangedDownload {
            part_size: PART_SIZE,
            parts_in_flight: PARTS_IN_FLIGHT,
        }
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

    async fn download_bytes(url: Url, config: RangedDownload) -> std::io::Result<Vec<u8>> {
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
        let config = RangedDownload {
            part_size: 1024 * 1024,
            ..config()
        };

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

    #[test]
    fn parts_cover_the_file_once() {
        let parts: Vec<_> = part_ranges(100, 1050, 400).collect();

        assert_eq!(parts, vec![0..100, 100..500, 500..900, 900..1050]);
    }
}
