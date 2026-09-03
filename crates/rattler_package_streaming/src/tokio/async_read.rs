//! Functions that enable extracting or streaming a Conda package for objects
//! that implement the [`tokio::io::AsyncRead`] trait.
//!
//! Extraction runs on a blocking worker thread. The async reader is pumped
//! into a bounded channel of fixed-size chunks, so downloading continues while
//! the worker decompresses and writes files with plain blocking I/O.

use std::{
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(feature = "reqwest")]
use futures_util::StreamExt;
use futures_util::future::{self, Either};
use rattler_digest::{Md5, Sha256, digest::Digest};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::{ExtractError, ExtractResult};

use super::shared::DEFAULT_BUF_SIZE;

/// Bytes per chunk handed to the extraction worker. Chunks are filled
/// completely before they are sent, so the worker wakes up once per chunk
/// rather than once per network read.
const CHUNK_SIZE: usize = DEFAULT_BUF_SIZE;

/// Chunks the download can run ahead of the extraction worker.
const CHUNKS_IN_FLIGHT: usize = 4;

/// Extracts the contents of a `.tar.bz2` package archive.
pub async fn extract_tar_bz2(
    reader: impl AsyncRead + Unpin,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    extract_blocking(reader, destination, |reader, destination| {
        crate::read::extract_tar_bz2_without_hashing(reader, destination)
    })
    .await
}

/// Extracts the contents of a `.conda` package archive, decompressing the
/// stream as it arrives.
pub async fn extract_conda(
    reader: impl AsyncRead + Unpin,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    extract_blocking(reader, destination, |reader, destination| {
        crate::read::extract_conda_via_streaming_without_hashing(reader, destination)
    })
    .await
}

/// Extracts the contents of a `.conda` package archive by fully reading the
/// stream before decompressing. This is the fallback for archives that cannot
/// be extracted while streaming, such as those using data descriptors.
pub async fn extract_conda_via_buffering(
    reader: impl AsyncRead + Unpin,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    extract_blocking(reader, destination, |reader, destination| {
        crate::read::extract_conda_via_buffering_without_hashing(reader, destination)
    })
    .await
}

/// A blocking reader over chunks received from the async pump.
struct ChunkReader {
    receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    position: usize,
    cancelled: Arc<AtomicBool>,
}

impl Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Stop at the next read once the extraction future is gone, instead
        // of draining buffered chunks into the destination first. Not
        // `Interrupted`: `io::copy` and `read_exact` retry on that kind.
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("extraction was cancelled"));
        }
        while self.position == self.chunk.len() {
            // A closed channel means the pump finished or was dropped; either
            // way there is no more data.
            let Some(chunk) = self.receiver.blocking_recv() else {
                return Ok(0);
            };
            self.chunk = chunk;
            self.position = 0;
        }
        let available = &self.chunk[self.position..];
        let len = available.len().min(buf.len());
        buf[..len].copy_from_slice(&available[..len]);
        self.position += len;
        Ok(len)
    }
}

/// Reads until `buf` is full or the reader reaches end-of-file. Returns the
/// number of bytes read.
async fn read_exact_or_eof<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut [u8],
) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let read = reader.read(&mut buf[filled..]).await?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

/// Sets the cancellation flag when dropped, so a worker whose future went
/// away stops at its next read.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn flatten_worker_result(
    joined: Result<Result<(), ExtractError>, tokio::task::JoinError>,
) -> Result<(), ExtractError> {
    match joined {
        Ok(result) => result,
        Err(err) => {
            if let Ok(reason) = err.try_into_panic() {
                std::panic::resume_unwind(reason);
            }
            Err(ExtractError::Cancelled)
        }
    }
}

/// Runs a blocking extractor on a worker thread while pumping and hashing
/// `reader` on the async runtime.
///
/// The worker reads to the end so the pump hashes the whole stream. If the
/// worker stops reading early, for example because the archive is invalid, the
/// channel closes and the pump stops. If the reader fails, the worker is told
/// to stop and awaited before the error is returned, so the destination is not
/// written to after this function returns. Dropping the returned future tells
/// the worker to stop at its next read; a write already in progress on the
/// worker thread finishes first.
///
/// Must be called from within a tokio runtime.
async fn extract_blocking<R: AsyncRead + Unpin>(
    reader: R,
    destination: &Path,
    extract: fn(Box<dyn Read + Send>, &Path) -> Result<(), ExtractError>,
) -> Result<ExtractResult, ExtractError> {
    let (sender, receiver) = tokio::sync::mpsc::channel(CHUNKS_IN_FLIGHT);
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_guard = CancelOnDrop(cancelled.clone());
    let chunks = ChunkReader {
        receiver,
        chunk: Vec::new(),
        position: 0,
        cancelled,
    };
    let destination = destination.to_path_buf();
    let mut worker = tokio::task::spawn_blocking(move || extract(Box::new(chunks), &destination));

    let pump = async move {
        let mut reader = reader;
        let mut sha256 = Sha256::new();
        let mut md5 = Md5::new();
        let mut total_size = 0;
        loop {
            let mut chunk = vec![0u8; CHUNK_SIZE];
            let read = read_exact_or_eof(&mut reader, &mut chunk).await?;
            if read == 0 {
                break;
            }
            sha256.update(&chunk[..read]);
            md5.update(&chunk[..read]);
            total_size += read as u64;
            chunk.truncate(read);
            // A closed channel means the worker stopped reading.
            if sender.send(chunk).await.is_err() || read < CHUNK_SIZE {
                break;
            }
        }
        // Dropping the sender signals end-of-file to the worker.
        if total_size == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "no data was read from the package stream - the stream may have been truncated",
            ));
        }
        Ok::<_, std::io::Error>(ExtractResult {
            sha256: sha256.finalize(),
            md5: md5.finalize(),
            total_size,
        })
    };

    // The pump is polled first so a read error is reported instead of the
    // truncated-archive error the worker produces when the channel closes
    // early.
    let pump = std::pin::pin!(pump);
    match future::select(pump, &mut worker).await {
        Either::Left((Ok(result), worker)) => {
            flatten_worker_result(worker.await)?;
            Ok(result)
        }
        Either::Left((Err(err), worker)) => {
            // The pump and its sender are gone. Stop the worker and wait for
            // it so nothing touches the destination after we return.
            drop(cancel_guard);
            let _ = worker.await;
            Err(ExtractError::IoError(err))
        }
        Either::Right((joined, pump)) => {
            flatten_worker_result(joined)?;
            pump.await.map_err(ExtractError::IoError)
        }
    }
}

/// Async equivalent of [`crate::seek::get_file_from_archive`].
///
/// Iterates entries of a tar archive, returning the contents of the first
/// entry whose path matches `file_name`. Because the reader is streaming,
/// only the bytes up to (and including) the target entry are consumed.
#[cfg(feature = "reqwest")]
pub(crate) async fn get_file_from_tar_archive<R: tokio::io::AsyncRead + Unpin>(
    archive: &mut tokio_tar::Archive<R>,
    file_name: &Path,
) -> Result<Option<Vec<u8>>, ExtractError> {
    let target = crate::archive::normalize(file_name)?;
    let mut entries = archive.entries().map_err(ExtractError::IoError)?;
    while let Some(entry) = entries.next().await {
        let mut entry = entry.map_err(ExtractError::IoError)?;
        let path = entry.path().map_err(ExtractError::IoError)?;
        // Normalized comparison, matching the sparse path in `crate::archive`.
        if crate::archive::normalize(&path)? == target {
            drop(path);
            return crate::archive::read_raw_entry_contents(&mut entry)
                .await
                .map(Some);
        }
    }
    Ok(None)
}
