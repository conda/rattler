//! This library provides the [`AsyncHttpRangeReader`] type.
//!
//! It allows streaming a file over HTTP while also allow random access. The type implements both
//! [`AsyncRead`] as well as [`AsyncSeek`]. This is supported through the use of range requests.
//! Each individual read will request a portion of the file using an HTTP range request.
//!
//! Requesting numerous small reads might turn out to be relatively slow because each reads needs to
//! perform an HTTP request. To alleviate this issue [`AsyncHttpRangeReader::prefetch`] is provided.
//! Using this method you can *prefect* a number of bytes which will be streamed in on the
//! background. If a read operation is reading from already (pre)fetched ranges it will stream from
//! the internal cache instead.
//!
//! Internally the [`AsyncHttpRangeReader`] stores a memory map which allows sparsely reading the
//! data into memory without actually requiring all memory for file to be resident in memory.
//!
//! The primary use-case for this library is to be able to sparsely stream a zip archive over HTTP
//! but its designed in a generic fashion.

mod sparse_range;

mod error;
#[cfg(test)]
mod static_directory_server;

use futures::{FutureExt, Stream, StreamExt};
use http_content_range::{ContentRange, ContentRangeBytes};
use memmap2::MmapMut;
use reqwest::header::HeaderMap;
use reqwest::{Client, Response, Url};
use sparse_range::SparseRange;
use std::{
    io::{self, ErrorKind, SeekFrom},
    ops::Range,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncSeek, ReadBuf},
    sync::watch::Sender,
    sync::{watch, Mutex},
};
use tokio_stream::wrappers::WatchStream;
use tokio_util::sync::PollSender;
use tracing::{info_span, Instrument};

pub use error::AsyncHttpRangeReaderError;

/// An `AsyncRangeReader` enables reading from a file over HTTP using range requests.
///
/// See the [`crate`] level documentation for more information.
#[derive(Debug)]
pub struct AsyncHttpRangeReader {
    inner: Mutex<Inner>,
    len: u64,
}

#[derive(Default, Clone, Debug)]
struct StreamerState {
    resident_range: SparseRange,
    requested_ranges: Vec<Range<u64>>,
    error: Option<AsyncHttpRangeReaderError>,
}

#[derive(Debug)]
struct Inner {
    /// A read-only view on the memory mapped data. The `downloaded_range` indicates the regions of
    /// memory that contain bytes that have been downloaded.
    data: &'static [u8],

    /// The current read position in the stream
    pos: u64,

    /// The range of bytes that have been requested for download
    requested_range: SparseRange,

    /// The range of bytes that have actually been downloaded to `data`.
    streamer_state: StreamerState,

    /// A channel receiver that holds the last downloaded range (or an error) from the background
    /// task.
    streamer_state_rx: WatchStream<StreamerState>,

    /// A channel sender to send range requests to the background task
    request_tx: tokio::sync::mpsc::Sender<Range<u64>>,

    /// An optional object to reserve a slot in the `request_tx` sender. When in the process of
    /// sending a requests this contains an actual value.
    poll_request_tx: Option<PollSender<Range<u64>>>,
}

pub enum CheckSupportMethod {
    // Perform a range request with a negative byte range. This will return the N bytes from the
    // *end* of the file as well as the file-size. This is especially useful to also immediately
    // get some bytes from the end of the file.
    NegativeRangeRequest(u64),

    // Perform a head request to get the length of the file and check if the server supports range
    // requests.
    Head,
}

impl AsyncHttpRangeReader {
    /// Construct a new `AsyncHttpRangeReader`.
    pub async fn new(
        client: reqwest::Client,
        url: reqwest::Url,
        check_method: CheckSupportMethod,
    ) -> Result<(Self, HeaderMap), AsyncHttpRangeReaderError> {
        match check_method {
            CheckSupportMethod::NegativeRangeRequest(initial_chunk_size) => {
                Self::new_tail_request(client, url, initial_chunk_size).await
            }
            CheckSupportMethod::Head => Self::new_head(client, url).await,
        }
    }

    /// An initial range request is performed to the server to determine if the remote accepts range
    /// requests. This will return a number of bytes from the end of the stream. Use the
    /// `initial_chunk_size` paramter to define how many bytes should be requested from the end.
    pub async fn new_tail_request(
        client: reqwest::Client,
        url: reqwest::Url,
        initial_chunk_size: u64,
    ) -> Result<(Self, HeaderMap), AsyncHttpRangeReaderError> {
        // Perform an initial range request to get the size of the file
        let tail_request_response = client
            .get(url.clone())
            .header(
                reqwest::header::RANGE,
                format!("bytes=-{initial_chunk_size}"),
            )
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .await
            .and_then(Response::error_for_status)
            .map_err(Arc::new)
            .map_err(AsyncHttpRangeReaderError::HttpError)?;
        let response_header = tail_request_response.headers().clone();

        // Get the size of the file from this initial request
        let content_range = ContentRange::parse(
            tail_request_response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .ok_or(AsyncHttpRangeReaderError::ContentRangeMissing)?
                .to_str()
                .map_err(|_| AsyncHttpRangeReaderError::ContentRangeMissing)?,
        );
        let (start, finish, complete_length) = match content_range {
            ContentRange::Bytes(ContentRangeBytes {
                first_byte,
                last_byte,
                complete_length,
            }) => (first_byte, last_byte, complete_length),
            _ => return Err(AsyncHttpRangeReaderError::HttpRangeRequestUnsupported),
        };

        // Allocate a memory map to hold the data
        let memory_map = memmap2::MmapOptions::new()
            .len(complete_length as usize)
            .map_anon()
            .map_err(Arc::new)
            .map_err(AsyncHttpRangeReaderError::MemoryMapError)?;

        // SAFETY: Get a read-only slice to the memory. This is safe because the memory map is never
        // reallocated and we keep track of the initialized part.
        let memory_map_slice =
            unsafe { std::slice::from_raw_parts(memory_map.as_ptr(), memory_map.len()) };

        let requested_range =
            SparseRange::from_range(complete_length - (finish - start)..complete_length);

        // adding more than 2 entries to the channel would block the sender. I assumed two would
        // suffice because I would want to 1) prefetch a certain range and 2) read stuff via the
        // AsyncRead implementation. Any extra would simply have to wait for one of these to
        // succeed. I eventually used 10 because who cares.
        let (request_tx, request_rx) = tokio::sync::mpsc::channel(10);
        let (state_tx, state_rx) = watch::channel(StreamerState::default());
        tokio::spawn(run_streamer(
            client,
            url,
            Some((tail_request_response, start)),
            memory_map,
            state_tx,
            request_rx,
        ));

        // Configure the initial state of the streamer.
        let mut streamer_state = StreamerState::default();
        streamer_state
            .requested_ranges
            .push(complete_length - (finish - start)..complete_length);

        let reader = Self {
            len: memory_map_slice.len() as u64,
            inner: Mutex::new(Inner {
                data: memory_map_slice,
                pos: 0,
                requested_range,
                streamer_state,
                streamer_state_rx: WatchStream::new(state_rx),
                request_tx,
                poll_request_tx: None,
            }),
        };
        Ok((reader, response_header))
    }

    async fn new_head(
        client: reqwest::Client,
        url: reqwest::Url,
    ) -> Result<(Self, HeaderMap), AsyncHttpRangeReaderError> {
        // Perform a HEAD request to get the content-length.
        let head_response = client
            .head(url.clone())
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .await
            .and_then(Response::error_for_status)
            .map_err(Arc::new)
            .map_err(AsyncHttpRangeReaderError::HttpError)?;

        // Are range requests supported?
        if head_response
            .headers()
            .get(reqwest::header::ACCEPT_RANGES)
            .and_then(|h| h.to_str().ok())
            != Some("bytes")
        {
            return Err(AsyncHttpRangeReaderError::HttpRangeRequestUnsupported);
        }

        let content_length: u64 = head_response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .ok_or(AsyncHttpRangeReaderError::ContentLengthMissing)?
            .to_str()
            .map_err(|_| AsyncHttpRangeReaderError::ContentLengthMissing)?
            .parse()
            .map_err(|_| AsyncHttpRangeReaderError::ContentLengthMissing)?;

        // Allocate a memory map to hold the data
        let memory_map = memmap2::MmapOptions::new()
            .len(content_length as _)
            .map_anon()
            .map_err(Arc::new)
            .map_err(AsyncHttpRangeReaderError::MemoryMapError)?;

        // SAFETY: Get a read-only slice to the memory. This is safe because the memory map is never
        // reallocated and we keep track of the initialized part.
        let memory_map_slice =
            unsafe { std::slice::from_raw_parts(memory_map.as_ptr(), memory_map.len()) };

        let requested_range = SparseRange::default();

        // adding more than 2 entries to the channel would block the sender. I assumed two would
        // suffice because I would want to 1) prefetch a certain range and 2) read stuff via the
        // AsyncRead implementation. Any extra would simply have to wait for one of these to
        // succeed. I eventually used 10 because who cares.
        let (request_tx, request_rx) = tokio::sync::mpsc::channel(10);
        let (state_tx, state_rx) = watch::channel(StreamerState::default());
        tokio::spawn(run_streamer(
            client, url, None, memory_map, state_tx, request_rx,
        ));

        // Configure the initial state of the streamer.
        let streamer_state = StreamerState::default();

        let reader = Self {
            len: memory_map_slice.len() as u64,
            inner: Mutex::new(Inner {
                data: memory_map_slice,
                pos: 0,
                requested_range,
                streamer_state,
                streamer_state_rx: WatchStream::new(state_rx),
                request_tx,
                poll_request_tx: None,
            }),
        };
        Ok((reader, head_response.headers().clone()))
    }

    /// Returns the ranges that this instance actually performed HTTP requests for.
    pub async fn requested_ranges(&self) -> Vec<Range<u64>> {
        let mut inner = self.inner.lock().await;
        if let Some(Some(new_state)) = inner.streamer_state_rx.next().now_or_never() {
            inner.streamer_state = new_state;
        }
        inner.streamer_state.requested_ranges.clone()
    }

    // Prefetches a range of bytes from the remote. When specifying a large range this can
    // drastically reduce the number of requests required to the server.
    pub async fn prefetch(&mut self, bytes: Range<u64>) {
        let inner = self.inner.get_mut();

        // Ensure the range is withing the file size and non-zero of length.
        let range = bytes.start..(bytes.end.min(inner.data.len() as u64));
        if range.start >= range.end {
            return;
        }

        // Check if the range has been requested or not.
        let inner = self.inner.get_mut();
        if let Some((new_range, _)) = inner.requested_range.cover(range.clone()) {
            let _ = inner.request_tx.send(range).await;
            inner.requested_range = new_range;
        }
    }

    /// Returns the length of the stream in bytes
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.len
    }
}

/// A task that will download parts from the remote archive and "send" them to the frontend as they
/// become available.
#[tracing::instrument(name = "fetch_ranges", skip_all, fields(url))]
async fn run_streamer(
    client: Client,
    url: Url,
    initial_tail_response: Option<(Response, u64)>,
    mut memory_map: MmapMut,
    mut state_tx: Sender<StreamerState>,
    mut request_rx: tokio::sync::mpsc::Receiver<Range<u64>>,
) {
    let mut state = StreamerState::default();

    if let Some((response, response_start)) = initial_tail_response {
        // Add the initial range to the state
        state
            .requested_ranges
            .push(response_start..memory_map.len() as u64);

        // Stream the initial data in memory
        if !stream_response(
            response,
            response_start,
            &mut memory_map,
            &mut state_tx,
            &mut state,
        )
        .await
        {
            return;
        }
    }

    // Listen for any new incoming requests
    'outer: loop {
        let range = match request_rx.recv().await {
            Some(range) => range,
            None => {
                break 'outer;
            }
        };

        // Determine the range that we need to cover
        let uncovered_ranges = match state.resident_range.cover(range) {
            None => continue,
            Some((_, uncovered_ranges)) => uncovered_ranges,
        };

        // Download and stream each range.
        for range in uncovered_ranges {
            // Update the requested ranges
            state
                .requested_ranges
                .push(*range.start()..*range.end() + 1);

            // Execute the request
            let range_string = format!("bytes={}-{}", range.start(), range.end());
            let span = info_span!("fetch_range", range = range_string.as_str());
            let response = match client
                .get(url.clone())
                .header(reqwest::header::RANGE, range_string)
                .header(reqwest::header::CACHE_CONTROL, "no-cache")
                .send()
                .instrument(span)
                .await
                .and_then(Response::error_for_status)
                .map_err(|e| std::io::Error::new(ErrorKind::Other, e))
            {
                Err(e) => {
                    state.error = Some(e.into());
                    let _ = state_tx.send(state);
                    break 'outer;
                }
                Ok(response) => response,
            };

            if !stream_response(
                response,
                *range.start(),
                &mut memory_map,
                &mut state_tx,
                &mut state,
            )
            .await
            {
                break 'outer;
            }
        }
    }
}

/// Streams the data from the specified response to the memory map updating progress in between.
/// Returns `true` if everything went fine, `false` if anything went wrong. The error state, if any,
/// is stored in `state_tx` so the "frontend" will consume it.
async fn stream_response(
    tail_request_response: Response,
    mut offset: u64,
    memory_map: &mut MmapMut,
    state_tx: &mut Sender<StreamerState>,
    state: &mut StreamerState,
) -> bool {
    let mut byte_stream = tail_request_response.bytes_stream();
    while let Some(bytes) = byte_stream.next().await {
        let bytes = match bytes {
            Err(e) => {
                state.error = Some(e.into());
                let _ = state_tx.send(state.clone());
                return false;
            }
            Ok(bytes) => bytes,
        };

        // Determine the range of these bytes in the complete file
        let byte_range = offset..offset + bytes.len() as u64;

        // Update the offset
        offset = byte_range.end;

        // Copy the data from the stream to memory
        memory_map[byte_range.start as usize..byte_range.end as usize]
            .copy_from_slice(bytes.as_ref());

        // Update the range of bytes that have been downloaded
        state.resident_range.update(byte_range);

        // Notify anyone that's listening that we have downloaded some extra data
        if state_tx.send(state.clone()).is_err() {
            // If we failed to set the state it means there is no receiver. In that case we should
            // just exit.
            return false;
        }
    }

    true
}

impl AsyncSeek for AsyncHttpRangeReader {
    fn start_seek(self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        let me = self.get_mut();
        let inner = me.inner.get_mut();

        inner.pos = match position {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(relative) => (inner.data.len() as i64).saturating_add(relative) as u64,
            SeekFrom::Current(relative) => (inner.pos as i64).saturating_add(relative) as u64,
        };

        Ok(())
    }

    fn poll_complete(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        let inner = self.inner.get_mut();
        Poll::Ready(Ok(inner.pos))
    }
}

impl AsyncRead for AsyncHttpRangeReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let inner = me.inner.get_mut();

        // If a previous error occurred we return that.
        if let Some(e) = inner.streamer_state.error.as_ref() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e.clone())));
        }

        // Determine the range to be fetched
        let range = inner.pos..(inner.pos + buf.remaining() as u64).min(inner.data.len() as u64);
        if range.start >= range.end {
            return Poll::Ready(Ok(()));
        }

        // Ensure we requested the required bytes
        while !inner.requested_range.is_covered(range.clone()) {
            // If there is an active range request wait for it to complete
            if let Some(mut poll) = inner.poll_request_tx.take() {
                match poll.poll_reserve(cx) {
                    Poll::Ready(_) => {
                        let _ = poll.send_item(range.clone());
                        inner.requested_range.update(range.clone());
                        break;
                    }
                    Poll::Pending => {
                        inner.poll_request_tx = Some(poll);
                        return Poll::Pending;
                    }
                }
            }

            // Request the range
            inner.poll_request_tx = Some(PollSender::new(inner.request_tx.clone()));
        }

        // If there is still a request poll open but there is no need for a request, abort it.
        if let Some(mut poll) = inner.poll_request_tx.take() {
            poll.abort_send();
        }

        loop {
            // Is the range already available?
            if inner
                .streamer_state
                .resident_range
                .is_covered(range.clone())
            {
                let len = (range.end - range.start) as usize;
                buf.initialize_unfilled_to(len)
                    .copy_from_slice(&inner.data[range.start as usize..range.end as usize]);
                buf.advance(len);
                inner.pos += len as u64;
                return Poll::Ready(Ok(()));
            }

            // Otherwise wait for new data to come in
            match ready!(Pin::new(&mut inner.streamer_state_rx).poll_next(cx)) {
                None => unreachable!(),
                Some(state) => {
                    inner.streamer_state = state;
                    if let Some(e) = inner.streamer_state.error.as_ref() {
                        return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e.clone())));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::static_directory_server::StaticDirectoryServer;
    use assert_matches::assert_matches;
    use async_zip::tokio::read::seek::ZipFileReader;
    use futures::AsyncReadExt;
    use reqwest::{Client, StatusCode};
    use rstest::*;
    use std::path::Path;
    use tokio::io::AsyncReadExt as _;
    use tokio_util::compat::TokioAsyncReadCompatExt;

    #[rstest]
    #[case(CheckSupportMethod::Head)]
    #[case(CheckSupportMethod::NegativeRangeRequest(8192))]
    #[tokio::test]
    async fn async_range_reader_zip(#[case] check_method: CheckSupportMethod) {
        // Spawn a static file server
        let path = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("test-data");
        let server = StaticDirectoryServer::new(&path);

        // check that file is there and has the right size
        let filepath = path.join("andes-1.8.3-pyhd8ed1ab_0.conda");
        assert!(
            filepath.exists(),
            "The conda package is not there yet. Did you run `git lfs pull`?"
        );
        let file_size = std::fs::metadata(&filepath).unwrap().len();
        assert_eq!(
            file_size, 2_463_995,
            "The conda package is not there yet. Did you run `git lfs pull`?"
        );

        // Construct an AsyncRangeReader
        let (mut range, _) = AsyncHttpRangeReader::new(
            Client::new(),
            server.url().join("andes-1.8.3-pyhd8ed1ab_0.conda").unwrap(),
            check_method,
        )
        .await
        .expect("Could not download range - did you run `git lfs pull`?");

        // Make sure we have read the last couple of bytes
        range.prefetch(range.len() - 8192..range.len()).await;

        assert_eq!(range.len(), file_size);

        let mut reader = ZipFileReader::new(range.compat()).await.unwrap();

        assert_eq!(
            reader
                .file()
                .entries()
                .iter()
                .map(|e| e.entry().filename().as_str().unwrap_or(""))
                .collect::<Vec<_>>(),
            vec![
                "metadata.json",
                "info-andes-1.8.3-pyhd8ed1ab_0.tar.zst",
                "pkg-andes-1.8.3-pyhd8ed1ab_0.tar.zst",
            ]
        );

        // Get the number of performed requests so far
        let request_ranges = reader.inner_mut().get_mut().requested_ranges().await;
        assert_eq!(request_ranges.len(), 1);
        assert_eq!(
            request_ranges[0].end - request_ranges[0].start,
            8192,
            "first request should be the size of the initial chunk size"
        );
        assert_eq!(
            request_ranges[0].end, file_size,
            "first request should be at the end"
        );

        // Prefetch the data for the metadata.json file
        let entry = reader.file().entries().get(0).unwrap();
        let offset = entry.header_offset();
        // Get the size of the entry plus the header + size of the filename. We should also actually
        // include bytes for the extra fields but we don't have that information.
        let size =
            entry.entry().compressed_size() + 30 + entry.entry().filename().as_bytes().len() as u64;

        // The zip archive uses as BufReader which reads in chunks of 8192. To ensure we prefetch
        // enough data we round the size up to the nearest multiple of the buffer size.
        let buffer_size = 8192;
        let size = ((size + buffer_size - 1) / buffer_size) * buffer_size;

        // Fetch the bytes from the zip archive that contain the requested file.
        reader
            .inner_mut()
            .get_mut()
            .prefetch(offset..offset + size as u64)
            .await;

        // Read the contents of the metadata.json file
        let mut contents = String::new();
        reader
            .reader_with_entry(0)
            .await
            .unwrap()
            .read_to_string(&mut contents)
            .await
            .unwrap();

        // Get the number of performed requests
        let request_ranges = reader.inner_mut().get_mut().requested_ranges().await;

        assert_eq!(contents, r#"{"conda_pkg_format_version": 2}"#);
        assert_eq!(request_ranges.len(), 2);
        assert_eq!(
            request_ranges[1],
            0..size,
            "expected only two range requests"
        );
    }

    #[rstest]
    #[case(CheckSupportMethod::Head)]
    #[case(CheckSupportMethod::NegativeRangeRequest(8192))]
    #[tokio::test]
    async fn async_range_reader(#[case] check_method: CheckSupportMethod) {
        // Spawn a static file server
        let path = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("test-data");
        let server = StaticDirectoryServer::new(&path);

        // Construct an AsyncRangeReader
        let (mut range, _) = AsyncHttpRangeReader::new(
            Client::new(),
            server.url().join("andes-1.8.3-pyhd8ed1ab_0.conda").unwrap(),
            check_method,
        )
        .await
        .expect("bla");

        // Also open a simple file reader
        let mut file = tokio::fs::File::open(path.join("andes-1.8.3-pyhd8ed1ab_0.conda"))
            .await
            .unwrap();

        // Read until the end and make sure that the contents matches
        let mut range_read = vec![0; 64 * 1024];
        let mut file_read = vec![0; 64 * 1024];
        loop {
            // Read with the async reader
            let range_read_bytes = range.read(&mut range_read).await.unwrap();

            // Read directly from the file
            let file_read_bytes = file
                .read_exact(&mut file_read[0..range_read_bytes])
                .await
                .unwrap();

            assert_eq!(range_read_bytes, file_read_bytes);
            assert_eq!(
                range_read[0..range_read_bytes],
                file_read[0..file_read_bytes]
            );

            if file_read_bytes == 0 && range_read_bytes == 0 {
                break;
            }
        }
    }

    #[tokio::test]
    async fn test_not_found() {
        let server = StaticDirectoryServer::new(Path::new(env!("CARGO_MANIFEST_DIR")));
        let err = AsyncHttpRangeReader::new(
            Client::new(),
            server.url().join("not-found").unwrap(),
            CheckSupportMethod::Head,
        )
        .await
        .expect_err("expected an error");

        assert_matches!(
            err, AsyncHttpRangeReaderError::HttpError(err) if err.status() == Some(StatusCode::NOT_FOUND)
        );
    }
}
