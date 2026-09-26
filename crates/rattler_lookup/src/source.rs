//! Random access to a layer file: a local file, an in-memory buffer, or a
//! remote file read with HTTP range requests.

use std::{
    io::SeekFrom,
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_http_range_reader::{AsyncHttpRangeReader, AsyncHttpRangeReaderError};
use bytes::Bytes;
use futures::{
    FutureExt,
    future::{BoxFuture, try_join_all},
};
use parquet::{
    arrow::{arrow_reader::ArrowReaderOptions, async_reader::AsyncFileReader},
    errors::ParquetError,
    file::metadata::ParquetMetaData,
};
use reqwest::header::HeaderMap;
use reqwest_middleware::ClientWithMiddleware;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::Mutex,
};
use url::Url;

use crate::{Location, LookupError};

/// How many bytes to request from the end of a remote file up front. This
/// covers the Parquet footer (and the offset index of a packages file), so
/// opening a file costs a single round trip.
pub const TAIL_SIZE: u64 = 64 * 1024;

/// Ranges closer together than this are fetched with a single request.
const COALESCE_GAP: u64 = 64 * 1024;

/// Requests and bytes read from a source.
#[derive(Debug, Default)]
pub struct Stats {
    requests: AtomicU64,
    bytes: AtomicU64,
}

impl Stats {
    /// The number of requests (or file reads) so far.
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    /// The number of bytes read so far.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    fn record(&self, bytes: u64) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// A file that can be read in byte ranges: a local file, an in-memory
/// buffer, or a remote file read with HTTP range requests.
pub struct ByteSource(Inner);

enum Inner {
    Local {
        path: PathBuf,
        file: Mutex<tokio::fs::File>,
        len: u64,
        stats: Stats,
    },
    Memory {
        location: Location,
        bytes: Bytes,
        stats: Stats,
    },
    Remote(Box<RemoteFile>),
}

/// A remote file read with HTTP range requests.
struct RemoteFile {
    client: ClientWithMiddleware,
    /// The URL as given.
    original: Url,
    /// The URL after following redirects (e.g. release assets that redirect
    /// to a signed blob URL), so later range requests skip the redirect.
    url: Url,
    len: u64,
    /// The reader created by the initial tail request; holds the tail.
    tail: Mutex<AsyncHttpRangeReader>,
    tail_start: u64,
    stats: Stats,
}

impl ByteSource {
    /// Opens a location. A remote file costs one request (which fetches the
    /// last [`TAIL_SIZE`] bytes), a local file none.
    pub async fn open(
        location: &Location,
        client: &ClientWithMiddleware,
    ) -> Result<Self, LookupError> {
        match location {
            Location::Url(url) => Ok(Self(Inner::Remote(Box::new(
                RemoteFile::open(client.clone(), url.clone()).await?,
            )))),
            Location::Path(path) => Self::open_local(path.clone()).await,
        }
    }

    async fn open_local(path: PathBuf) -> Result<Self, LookupError> {
        let location = Location::Path(path.clone());
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| LookupError::io(&location, e))?;
        let len = file
            .metadata()
            .await
            .map_err(|e| LookupError::io(&location, e))?
            .len();
        Ok(Self(Inner::Local {
            path,
            file: Mutex::new(file),
            len,
            stats: Stats::default(),
        }))
    }

    /// A source over bytes that are already in memory.
    pub fn from_bytes(location: Location, bytes: Bytes) -> Self {
        Self(Inner::Memory {
            location,
            bytes,
            stats: Stats::default(),
        })
    }

    /// The size of the file in bytes.
    pub fn len(&self) -> u64 {
        match &self.0 {
            Inner::Local { len, .. } => *len,
            Inner::Memory { bytes, .. } => bytes.len() as u64,
            Inner::Remote(remote) => remote.len,
        }
    }

    /// Whether the file is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Requests and bytes read so far.
    pub fn stats(&self) -> &Stats {
        match &self.0 {
            Inner::Local { stats, .. } | Inner::Memory { stats, .. } => stats,
            Inner::Remote(remote) => &remote.stats,
        }
    }

    /// Where the file lives, for error messages.
    pub fn location(&self) -> Location {
        match &self.0 {
            Inner::Local { path, .. } => Location::Path(path.clone()),
            Inner::Memory { location, .. } => location.clone(),
            Inner::Remote(remote) => Location::Url(remote.original.clone()),
        }
    }

    /// Fetches all ranges, issuing requests concurrently where needed. Nearby
    /// ranges are coalesced into one request.
    pub async fn fetch(&self, ranges: &[Range<u64>]) -> Result<Vec<Bytes>, LookupError> {
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        let len = self.len();
        for range in ranges {
            if range.start > range.end || range.end > len {
                return Err(LookupError::invalid_file(
                    &self.location(),
                    format!("byte range {range:?} is out of bounds for a file of {len} bytes"),
                ));
            }
        }

        let mut order: Vec<usize> = (0..ranges.len()).collect();
        order.sort_by_key(|&i| ranges[i].start);
        let mut merged: Vec<Range<u64>> = Vec::new();
        for &i in &order {
            let range = &ranges[i];
            match merged.last_mut() {
                Some(last) if range.start <= last.end + COALESCE_GAP => {
                    last.end = last.end.max(range.end);
                }
                _ => merged.push(range.clone()),
            }
        }

        let buffers =
            try_join_all(merged.iter().map(|range| self.fetch_one(range.clone()))).await?;

        Ok(ranges
            .iter()
            .map(|range| {
                let idx = merged
                    .partition_point(|m| m.start <= range.start)
                    .saturating_sub(1);
                let base = merged[idx].start;
                buffers[idx].slice((range.start - base) as usize..(range.end - base) as usize)
            })
            .collect())
    }

    async fn fetch_one(&self, range: Range<u64>) -> Result<Bytes, LookupError> {
        match &self.0 {
            Inner::Local {
                path, file, stats, ..
            } => {
                let mut buf = vec![0u8; (range.end - range.start) as usize];
                let location = Location::Path(path.clone());
                let mut file = file.lock().await;
                file.seek(SeekFrom::Start(range.start))
                    .await
                    .map_err(|e| LookupError::io(&location, e))?;
                file.read_exact(&mut buf)
                    .await
                    .map_err(|e| LookupError::io(&location, e))?;
                stats.record(buf.len() as u64);
                Ok(buf.into())
            }
            Inner::Memory { bytes, stats, .. } => {
                stats.record(range.end - range.start);
                Ok(bytes.slice(range.start as usize..range.end as usize))
            }
            Inner::Remote(remote) => remote.read(range).await,
        }
    }
}

impl RemoteFile {
    async fn open(client: ClientWithMiddleware, url: Url) -> Result<Self, LookupError> {
        let stats = Stats::default();
        let http_error = |source: Arc<reqwest_middleware::Error>| LookupError::Http {
            url: url.clone(),
            source,
        };

        // Fast path: a suffix range request returns the file size and the
        // footer in a single round trip.
        let tail = AsyncHttpRangeReader::initial_tail_request(
            client.clone(),
            url.clone(),
            TAIL_SIZE,
            HeaderMap::new(),
        )
        .await;
        stats.requests.fetch_add(1, Ordering::Relaxed);

        let (reader, resolved) = match tail {
            Ok(response) => {
                let resolved = response.url().clone();
                let reader = AsyncHttpRangeReader::from_range_response(
                    client.clone(),
                    response,
                    resolved.clone(),
                    HeaderMap::new(),
                )
                .await
                .map_err(|source| LookupError::RangeRequestsUnsupported {
                    url: url.clone(),
                    source: Box::new(source),
                })?;
                (reader, resolved)
            }
            Err(AsyncHttpRangeReaderError::HttpError(err)) => {
                // Some servers reject suffix ranges (`416` when the range
                // exceeds the file, `501 Unsupported client range` on GitHub
                // release assets). Fall back to a HEAD request plus an
                // explicit range, reusing the URL we were redirected to.
                let status = match err.as_ref() {
                    reqwest_middleware::Error::Reqwest(e) => e.status(),
                    reqwest_middleware::Error::Middleware(_) => None,
                };
                let retry_without_suffix = matches!(
                    status,
                    Some(
                        reqwest::StatusCode::RANGE_NOT_SATISFIABLE
                            | reqwest::StatusCode::NOT_IMPLEMENTED
                            | reqwest::StatusCode::BAD_REQUEST
                    )
                );
                if !retry_without_suffix {
                    return Err(http_error(err));
                }
                let resolved = match err.as_ref() {
                    reqwest_middleware::Error::Reqwest(e) => e.url().cloned(),
                    reqwest_middleware::Error::Middleware(_) => None,
                }
                .unwrap_or_else(|| url.clone());
                let head = AsyncHttpRangeReader::initial_head_request(
                    client.clone(),
                    resolved.clone(),
                    HeaderMap::new(),
                )
                .await
                .map_err(|e| match e {
                    AsyncHttpRangeReaderError::HttpError(e) => http_error(e),
                    source => LookupError::RangeRequestsUnsupported {
                        url: url.clone(),
                        source: Box::new(source),
                    },
                })?;
                stats.requests.fetch_add(1, Ordering::Relaxed);
                let resolved = head.url().clone();
                let mut reader = AsyncHttpRangeReader::from_head_response(
                    client.clone(),
                    head,
                    resolved.clone(),
                    HeaderMap::new(),
                )
                .await
                .map_err(|source| LookupError::RangeRequestsUnsupported {
                    url: url.clone(),
                    source: Box::new(source),
                })?;
                let len = reader.len();
                reader.prefetch(len.saturating_sub(TAIL_SIZE)..len).await;
                stats.requests.fetch_add(1, Ordering::Relaxed);
                (reader, resolved)
            }
            Err(source @ AsyncHttpRangeReaderError::HttpRangeRequestUnsupported) => {
                return Err(LookupError::RangeRequestsUnsupported {
                    url,
                    source: Box::new(source),
                });
            }
            Err(source) => {
                return Err(LookupError::RangeRequest {
                    url,
                    range: 0..TAIL_SIZE,
                    source: Box::new(source),
                });
            }
        };

        let len = reader.len();
        stats.bytes.fetch_add(TAIL_SIZE.min(len), Ordering::Relaxed);
        Ok(Self {
            client,
            original: url,
            url: resolved,
            len,
            tail: Mutex::new(reader),
            tail_start: len.saturating_sub(TAIL_SIZE),
            stats,
        })
    }

    async fn read(&self, range: Range<u64>) -> Result<Bytes, LookupError> {
        let mut buf = vec![0u8; (range.end - range.start) as usize];
        let range_error = |source| LookupError::RangeRequest {
            url: self.original.clone(),
            range: range.clone(),
            source: Box::new(source),
        };
        let io_error = |e| range_error(AsyncHttpRangeReaderError::IoError(Arc::new(e)));

        if range.start >= self.tail_start {
            // Already resident in memory from the initial request.
            let mut reader = self.tail.lock().await;
            reader
                .seek(SeekFrom::Start(range.start))
                .await
                .map_err(io_error)?;
            reader.read_exact(&mut buf).await.map_err(io_error)?;
            return Ok(buf.into());
        }

        // An `AsyncHttpRangeReader` downloads requested ranges one after the
        // other. To fetch independent ranges concurrently, every range gets
        // its own reader. Since the file size is known, it is created from a
        // synthesized HEAD response without touching the network.
        let head = http::Response::builder()
            .header(reqwest::header::ACCEPT_RANGES, "bytes")
            .header(reqwest::header::CONTENT_LENGTH, self.len)
            .body(Vec::<u8>::new())
            .expect("a valid synthetic response");
        let mut reader = AsyncHttpRangeReader::from_head_response(
            self.client.clone(),
            reqwest::Response::from(head),
            self.url.clone(),
            HeaderMap::new(),
        )
        .await
        .map_err(range_error)?;
        reader.prefetch(range.clone()).await;
        reader
            .seek(SeekFrom::Start(range.start))
            .await
            .map_err(io_error)?;
        reader.read_exact(&mut buf).await.map_err(io_error)?;
        self.stats.record(buf.len() as u64);
        Ok(buf.into())
    }
}

/// Adapter so the `parquet` crate's async reader can read from a
/// [`ByteSource`], preferring byte ranges that were prefetched.
#[derive(Clone)]
pub(crate) struct ParquetSource {
    pub source: Arc<ByteSource>,
    pub metadata: Arc<ParquetMetaData>,
    /// Prefetched byte ranges.
    pub cache: Arc<Vec<(Range<u64>, Bytes)>>,
}

impl ParquetSource {
    async fn read(&self, ranges: Vec<Range<u64>>) -> parquet::errors::Result<Vec<Bytes>> {
        let cached = |range: &Range<u64>| {
            self.cache.iter().find_map(|(r, bytes)| {
                (r.start <= range.start && range.end <= r.end).then(|| {
                    bytes.slice((range.start - r.start) as usize..(range.end - r.start) as usize)
                })
            })
        };
        let mut result: Vec<Option<Bytes>> = ranges.iter().map(cached).collect();
        let missing: Vec<Range<u64>> = ranges
            .iter()
            .zip(&result)
            .filter(|(_, bytes)| bytes.is_none())
            .map(|(range, _)| range.clone())
            .collect();
        if !missing.is_empty() {
            let mut fetched = self
                .source
                .fetch(&missing)
                .await
                .map_err(|e| ParquetError::External(e.into()))?
                .into_iter();
            for slot in result.iter_mut().filter(|slot| slot.is_none()) {
                *slot = fetched.next();
            }
        }
        Ok(result.into_iter().map(|b| b.expect("filled")).collect())
    }
}

impl AsyncFileReader for ParquetSource {
    fn get_bytes(&mut self, range: Range<u64>) -> BoxFuture<'_, parquet::errors::Result<Bytes>> {
        async move { Ok(self.read(vec![range]).await?.pop().expect("one range")) }.boxed()
    }

    fn get_byte_ranges(
        &mut self,
        ranges: Vec<Range<u64>>,
    ) -> BoxFuture<'_, parquet::errors::Result<Vec<Bytes>>> {
        async move { self.read(ranges).await }.boxed()
    }

    fn get_metadata<'a>(
        &'a mut self,
        _options: Option<&'a ArrowReaderOptions>,
    ) -> BoxFuture<'a, parquet::errors::Result<Arc<ParquetMetaData>>> {
        let metadata = self.metadata.clone();
        async move { Ok(metadata) }.boxed()
    }
}
