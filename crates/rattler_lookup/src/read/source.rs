//! Random access to a local file or a remote file over HTTP range requests.

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

use crate::{LookupError, Result};

/// How many bytes to request from the end of a remote file up front. This
/// covers the Parquet footer, so opening a file costs a single round trip.
pub const TAIL_SIZE: u64 = 64 * 1024;

/// Ranges closer together than this are fetched with a single request.
const COALESCE_GAP: u64 = 64 * 1024;

/// How many requests were issued and how many bytes were read.
#[derive(Debug, Default)]
pub struct Stats {
    /// The number of requests (or reads, for a local file).
    pub requests: AtomicU64,
    /// The number of bytes read.
    pub bytes: AtomicU64,
}

/// A file that supports random access, either locally or over HTTP.
pub struct ByteSource {
    url: Url,
    kind: Kind,
    len: u64,
    stats: Stats,
}

enum Kind {
    Local {
        path: PathBuf,
        file: Mutex<tokio::fs::File>,
    },
    Remote(Box<RemoteFile>),
}

struct RemoteFile {
    client: ClientWithMiddleware,
    /// The url after following redirects, so later range requests skip the
    /// redirect.
    url: Url,
    len: u64,
    /// The reader created by the initial tail request; holds the file tail.
    tail: Mutex<AsyncHttpRangeReader>,
    tail_start: u64,
}

impl ByteSource {
    /// Opens an `http(s)://` or `file://` url for random access.
    pub async fn open(url: &Url, client: &ClientWithMiddleware) -> Result<Self> {
        match url.scheme() {
            "http" | "https" => {
                let stats = Stats::default();
                let remote = RemoteFile::open(client.clone(), url, &stats).await?;
                Ok(Self {
                    url: url.clone(),
                    len: remote.len,
                    kind: Kind::Remote(Box::new(remote)),
                    stats,
                })
            }
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|()| LookupError::InvalidFileUrl(Box::new(url.clone())))?;
                let file =
                    tokio::fs::File::open(&path)
                        .await
                        .map_err(|source| LookupError::Io {
                            path: path.clone(),
                            source,
                        })?;
                let len = file
                    .metadata()
                    .await
                    .map_err(|source| LookupError::Io {
                        path: path.clone(),
                        source,
                    })?
                    .len();
                Ok(Self {
                    url: url.clone(),
                    kind: Kind::Local {
                        path,
                        file: Mutex::new(file),
                    },
                    len,
                    stats: Stats::default(),
                })
            }
            scheme => Err(LookupError::UnsupportedScheme(scheme.to_string())),
        }
    }

    /// The url this source was opened from.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The size of the file in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// How much was read from this source so far.
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Fetches all ranges, issuing requests concurrently where needed.
    pub async fn fetch(&self, ranges: &[Range<u64>]) -> Result<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        for range in ranges {
            if range.start > range.end || range.end > self.len {
                return Err(LookupError::OutOfBounds {
                    url: Box::new(self.url.clone()),
                    start: range.start,
                    end: range.end,
                    len: self.len,
                });
            }
        }

        // Coalesce nearby ranges so we don't issue lots of tiny requests.
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
                    .checked_sub(1)
                    .expect("every range is covered by a merged range");
                let base = merged[idx].start;
                buffers[idx].slice((range.start - base) as usize..(range.end - base) as usize)
            })
            .collect())
    }

    async fn fetch_one(&self, range: Range<u64>) -> Result<Bytes> {
        let mut buf = vec![0u8; (range.end - range.start) as usize];
        match &self.kind {
            Kind::Local { path, file } => {
                let mut file = file.lock().await;
                let mut read = async || {
                    file.seek(SeekFrom::Start(range.start)).await?;
                    file.read_exact(&mut buf).await
                };
                read().await.map_err(|source| LookupError::Io {
                    path: path.clone(),
                    source,
                })?;
            }
            Kind::Remote(remote) => remote.read(range, &mut buf).await?,
        }
        self.stats.requests.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes
            .fetch_add(buf.len() as u64, Ordering::Relaxed);
        Ok(buf.into())
    }
}

impl RemoteFile {
    async fn open(client: ClientWithMiddleware, url: &Url, stats: &Stats) -> Result<Self> {
        let range_error = |source| LookupError::RangeRequests {
            url: Box::new(url.clone()),
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
                .map_err(range_error)?;
                (reader, resolved)
            }
            Err(AsyncHttpRangeReaderError::HttpError(err)) => {
                // Some servers reject suffix ranges (GitHub release assets
                // answer `501 Unsupported client range`). Fall back to a HEAD
                // request, reusing the url we were redirected to.
                let resolved = match err.as_ref() {
                    reqwest_middleware::Error::Reqwest(err) => err.url().cloned(),
                    reqwest_middleware::Error::Middleware(_) => None,
                }
                .unwrap_or_else(|| url.clone());
                let head = AsyncHttpRangeReader::initial_head_request(
                    client.clone(),
                    resolved,
                    HeaderMap::new(),
                )
                .await
                .map_err(range_error)?;
                stats.requests.fetch_add(1, Ordering::Relaxed);
                let resolved = head.url().clone();
                let mut reader = AsyncHttpRangeReader::from_head_response(
                    client.clone(),
                    head,
                    resolved.clone(),
                    HeaderMap::new(),
                )
                .await
                .map_err(range_error)?;
                let len = reader.len();
                reader.prefetch(len.saturating_sub(TAIL_SIZE)..len).await;
                stats.requests.fetch_add(1, Ordering::Relaxed);
                (reader, resolved)
            }
            Err(err) => return Err(range_error(err)),
        };

        let len = reader.len();
        stats.bytes.fetch_add(TAIL_SIZE.min(len), Ordering::Relaxed);
        Ok(Self {
            client,
            url: resolved,
            len,
            tail: Mutex::new(reader),
            tail_start: len.saturating_sub(TAIL_SIZE),
        })
    }

    async fn read(&self, range: Range<u64>, buf: &mut [u8]) -> Result<()> {
        let read_error = |source| LookupError::RemoteRead {
            url: Box::new(self.url.clone()),
            start: range.start,
            end: range.end,
            source,
        };

        if range.start >= self.tail_start {
            // Already resident in memory from the initial request.
            let mut reader = self.tail.lock().await;
            reader
                .seek(SeekFrom::Start(range.start))
                .await
                .map_err(read_error)?;
            reader.read_exact(buf).await.map_err(read_error)?;
            return Ok(());
        }

        // An `AsyncHttpRangeReader` downloads requested ranges one after the
        // other. To fetch independent ranges concurrently, every range gets its
        // own reader. Since we already know the file size, it is created from a
        // synthesized HEAD response without touching the network.
        let head = http::Response::builder()
            .header(reqwest::header::ACCEPT_RANGES, "bytes")
            .header(reqwest::header::CONTENT_LENGTH, self.len)
            .body(Vec::<u8>::new())
            .expect("a response with two valid headers");
        let mut reader = AsyncHttpRangeReader::from_head_response(
            self.client.clone(),
            reqwest::Response::from(head),
            self.url.clone(),
            HeaderMap::new(),
        )
        .await
        .map_err(|source| LookupError::RangeRequests {
            url: Box::new(self.url.clone()),
            source,
        })?;
        reader.prefetch(range.clone()).await;
        reader
            .seek(SeekFrom::Start(range.start))
            .await
            .map_err(read_error)?;
        reader.read_exact(buf).await.map_err(read_error)?;
        Ok(())
    }
}

/// Adapter so the `parquet` crate's async reader can read from a [`ByteSource`].
#[derive(Clone)]
pub struct ParquetSource {
    pub source: Arc<ByteSource>,
    pub metadata: Arc<ParquetMetaData>,
    /// Prefetched byte ranges.
    pub cache: Arc<Vec<(Range<u64>, Bytes)>>,
}

impl ParquetSource {
    async fn read(&self, ranges: Vec<Range<u64>>) -> parquet::errors::Result<Vec<Bytes>> {
        let cached = |range: &Range<u64>| {
            self.cache.iter().find_map(|(cached, bytes)| {
                (cached.start <= range.start && range.end <= cached.end).then(|| {
                    bytes.slice(
                        (range.start - cached.start) as usize..(range.end - cached.start) as usize,
                    )
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
                .map_err(|err| ParquetError::External(err.into()))?
                .into_iter();
            for slot in result.iter_mut().filter(|slot| slot.is_none()) {
                *slot = fetched.next();
            }
        }
        Ok(result
            .into_iter()
            .map(|bytes| bytes.expect("every range was either cached or fetched"))
            .collect())
    }
}

impl AsyncFileReader for ParquetSource {
    fn get_bytes(&mut self, range: Range<u64>) -> BoxFuture<'_, parquet::errors::Result<Bytes>> {
        async move {
            Ok(self
                .read(vec![range])
                .await?
                .pop()
                .expect("one range was requested"))
        }
        .boxed()
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
