mod sparse_range;

#[cfg(test)]
mod static_directory_server;

use futures::{Stream, StreamExt};
use http_content_range::{ContentRange, ContentRangeBytes};
use memmap2::MmapMut;
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

/// An `AsyncRangeReader` enables reading from a file over HTTP using range requests.
#[derive(Debug)]
pub struct AsyncHttpRangeReader {
    inner: Mutex<Inner>,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum AsyncHttpRangeReaderError {
    #[error("range requests are not supported")]
    HttpRangeRequestUnsupported,

    #[error(transparent)]
    HttpError(#[from] Arc<reqwest::Error>),

    #[error("an error occurred during transport: {0}")]
    TransportError(#[source] Arc<reqwest::Error>),

    #[error("io error occurred: {0}")]
    IoError(#[source] Arc<std::io::Error>),

    #[error("content-range header is missing from response")]
    ContentRangeMissing,

    #[error("memory mapping the file failed")]
    MemoryMapError(#[source] Arc<std::io::Error>),
}

impl From<std::io::Error> for AsyncHttpRangeReaderError {
    fn from(err: std::io::Error) -> Self {
        AsyncHttpRangeReaderError::IoError(Arc::new(err))
    }
}

impl From<reqwest::Error> for AsyncHttpRangeReaderError {
    fn from(err: reqwest::Error) -> Self {
        AsyncHttpRangeReaderError::TransportError(Arc::new(err))
    }
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
    downloaded_range: Result<SparseRange, AsyncHttpRangeReaderError>,

    /// A channel receiver that holds the last downloaded range (or an error) from the background
    /// task.
    state_rx: WatchStream<Result<SparseRange, AsyncHttpRangeReaderError>>,

    /// A channel sender to send range requests to the background task
    request_tx: tokio::sync::mpsc::Sender<Range<u64>>,

    /// An optional object to reserve a slot in the `request_tx` sender. When in the process of
    /// sending a requests this contains an actual value.
    poll_request_tx: Option<PollSender<Range<u64>>>,
}

impl AsyncHttpRangeReader {
    /// Construct a new `AsyncHttpRangeReader`.
    pub async fn new(
        client: reqwest::Client,
        url: reqwest::Url,
    ) -> Result<Self, AsyncHttpRangeReaderError> {
        // Perform an initial range request to get the size of the file
        const INITIAL_CHUNK_SIZE: usize = 16384;
        let tail_request_response = client
            .get(url.clone())
            .header(
                reqwest::header::RANGE,
                format!("bytes=-{INITIAL_CHUNK_SIZE}"),
            )
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .await
            .and_then(Response::error_for_status)
            .map_err(Arc::new)
            .map_err(AsyncHttpRangeReaderError::HttpError)?;
        let tail_request_response = if tail_request_response.status() != 206 {
            return Err(AsyncHttpRangeReaderError::HttpRangeRequestUnsupported);
        } else {
            tail_request_response.error_for_status()?
        };

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
        let (state_tx, state_rx) = watch::channel(Ok(SparseRange::new()));
        tokio::spawn(run_streamer(
            client,
            url,
            tail_request_response,
            start,
            memory_map,
            state_tx,
            request_rx,
        ));

        Ok(Self {
            inner: Mutex::new(Inner {
                data: memory_map_slice,
                pos: 0,
                requested_range,
                downloaded_range: Ok(SparseRange::new()),
                state_rx: WatchStream::new(state_rx),
                request_tx,
                poll_request_tx: None,
            }),
        })
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
}

/// A task that will download parts from the remote archive and "send" them to the frontend as they
/// become available.
#[tracing::instrument(name = "fetch_ranges", skip_all, fields(url))]
async fn run_streamer(
    client: Client,
    url: Url,
    response: Response,
    response_start: u64,
    mut memory_map: MmapMut,
    mut state_tx: Sender<Result<SparseRange, AsyncHttpRangeReaderError>>,
    mut request_rx: tokio::sync::mpsc::Receiver<Range<u64>>,
) {
    let mut downloaded_range = SparseRange::new();

    // Stream the initial data in memory
    if !stream_response(
        response,
        response_start,
        &mut memory_map,
        &mut state_tx,
        &mut downloaded_range,
    )
    .await
    {
        return;
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
        let uncovered_ranges = match downloaded_range.cover(range) {
            None => continue,
            Some((_, uncovered_ranges)) => uncovered_ranges,
        };

        // Download and stream each range.
        for range in uncovered_ranges {
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
                    let _ = state_tx.send(Err(e.into()));
                    break 'outer;
                }
                Ok(response) => response,
            };

            if !stream_response(
                response,
                *range.start(),
                &mut memory_map,
                &mut state_tx,
                &mut downloaded_range,
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
    state_tx: &mut Sender<Result<SparseRange, AsyncHttpRangeReaderError>>,
    downloaded_range: &mut SparseRange,
) -> bool {
    let mut byte_stream = tail_request_response.bytes_stream();
    while let Some(bytes) = byte_stream.next().await {
        let bytes = match bytes {
            Err(e) => {
                let _ = state_tx.send(Err(e.into()));
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
        downloaded_range.update(byte_range);

        // Notify anyone that's listening that we have downloaded some extra data
        if state_tx.send(Ok(downloaded_range.clone())).is_err() {
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
        if let Err(e) = &inner.downloaded_range {
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
                .downloaded_range
                .as_ref()
                .unwrap()
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
            match ready!(Pin::new(&mut inner.state_rx).poll_next(cx)) {
                None => unreachable!(),
                Some(Err(e)) => {
                    inner.downloaded_range = Err(e.clone());
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e)));
                }
                Some(Ok(range)) => {
                    inner.downloaded_range = Ok(range);
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
    use std::path::Path;
    use tokio::io::AsyncReadExt as _;
    use tokio_util::compat::TokioAsyncReadCompatExt;

    #[tokio::test]
    async fn async_range_reader_zip() {
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
        let range = AsyncHttpRangeReader::new(
            Client::new(),
            server.url().join("andes-1.8.3-pyhd8ed1ab_0.conda").unwrap(),
        )
        .await
        .expect("Could not download range - did you run `git lfs pull`?");

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
                "pkg-andes-1.8.3-pyhd8ed1ab_0.tar.zst"
            ]
        );

        // Prefetch the data for the metadata.json file
        let entry = reader.file().entries().get(0).unwrap();
        let offset = entry.header_offset();
        // Get the size of the entry plus the header + size of the filename. We should also actually
        // include bytes for the extra fields but we don't have that information.
        let size =
            entry.entry().compressed_size() + 30 + entry.entry().filename().as_bytes().len() as u64;
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

        assert_eq!(contents, r#"{"conda_pkg_format_version": 2}"#);
    }

    #[tokio::test]
    async fn async_range_reader() {
        // Spawn a static file server
        let path = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("test-data");
        let server = StaticDirectoryServer::new(&path);

        // Construct an AsyncRangeReader
        let mut range = AsyncHttpRangeReader::new(
            Client::new(),
            server.url().join("andes-1.8.3-pyhd8ed1ab_0.conda").unwrap(),
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
        let err = AsyncHttpRangeReader::new(Client::new(), server.url().join("not-found").unwrap())
            .await
            .expect_err("expected an error");

        assert_matches!(
            err, AsyncHttpRangeReaderError::HttpError(err) if err.status() == Some(StatusCode::NOT_FOUND)
        );
    }
}
