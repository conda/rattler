//! Hash `ETag`-guarded archives with one sequential GET and read their info component
//! with bounded ranges. Unversioned storage and unsupported ZIP layouts use a buffered read.

use std::{
    collections::HashSet,
    error::Error as StdError,
    io::{self, Write},
};

use async_zip::{Compression, base::read::seek::ZipFileReader, error::ZipError};
use bytes::Buf;
use futures::{AsyncBufReadExt, AsyncSeekExt};
use opendal::{Buffer, BufferCursor, Error, ErrorKind, Operator, Reader, options::ReaderOptions};
use rattler_digest::{HashingWriter, Md5, Sha256};
use rattler_package_streaming::seek::stream_conda_info;
use tar::Archive;
use tokio::task::{consume_budget, spawn_blocking};
use zstd::stream::read::Decoder;

use crate::{
    ArchiveDigests, IndexedPackageRecord, RepodataFileMetadata, read_indexed_json_from_archive,
    utils::read_with_metadata_check,
};

const METADATA_CHUNK: usize = 64 * 1024;
const HASH_CHUNK: usize = 8 * 1024;

/// Collect both whole-archive hashes and the byte count in one pass.
struct ArchiveHasher {
    writer: HashingWriter<HashingWriter<io::Sink, Sha256>, Md5>,
    size: u64,
}

impl ArchiveHasher {
    /// Discard payload bytes after feeding both digest writers.
    fn new() -> Self {
        Self {
            writer: HashingWriter::new(HashingWriter::new(io::sink())),
            size: 0,
        }
    }

    /// Include a chunk in both digests and the final archive size.
    fn update(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.size += bytes.len() as u64;
        Ok(())
    }

    /// Supply all fields required to construct a complete package record.
    fn finish(self) -> ArchiveDigests {
        let (sha256, md5) = self.writer.finalize();
        let (_, sha256) = sha256.finalize();
        ArchiveDigests {
            md5,
            sha256,
            size: self.size,
        }
    }
}

/// Distinguish ZIP fallback decisions from storage errors that may need a retry.
#[derive(Debug)]
enum MetadataReadError {
    UnsupportedLayout,
    Storage(Error),
}

impl From<Error> for MetadataReadError {
    fn from(error: Error) -> Self {
        Self::Storage(error)
    }
}

/// Return metadata and digests from one archive version, falling back to a buffered read.
/// Conditional reads rely on the backend enforcing `If-Match`; disabled conditions
/// use an `ETag` check after parsing, and replacement errors must retry the whole operation.
pub(crate) async fn read_package(
    op: &Operator,
    path: &str,
    metadata: &RepodataFileMetadata,
) -> opendal::Result<IndexedPackageRecord> {
    // Timestamps cannot establish identity across separate reads.
    if metadata.etag.is_none() {
        return buffered_record(op, path, metadata).await;
    }
    let result = read_version(op, path, metadata).await;
    // A replacement can invalidate ZIP offsets, so failed reads need validation too.
    if !metadata.precondition_checks.is_enabled() {
        let after = op.stat(path).await?;
        if metadata.etag.as_deref() != after.etag() {
            return Err(Error::new(
                ErrorKind::ConditionNotMatch,
                "archive changed while indexing",
            ));
        }
    }
    result
}

/// Keep attempt errors pending until the caller can validate unguarded reads.
async fn read_version(
    op: &Operator,
    path: &str,
    metadata: &RepodataFileMetadata,
) -> opendal::Result<IndexedPackageRecord> {
    let mut options = ReaderOptions::default();
    if metadata.precondition_checks.is_enabled() {
        options.if_match = metadata.etag.clone();
    }
    let hash_reader = op.reader_options(path, options.clone()).await?;
    let mut source = hash_reader.clone().into_futures_async_read(..).await?;
    let mut hasher = ArchiveHasher::new();
    loop {
        let chunk = source.fill_buf().await.map_err(storage_error)?;
        if chunk.is_empty() {
            break;
        }
        // Check the cooperative budget every 8 KiB so ready buffers cannot starve the executor.
        let chunk = &chunk[..chunk.len().min(HASH_CHUNK)];
        hasher.update(chunk).map_err(storage_error)?;
        let length = chunk.len();
        source.consume_unpin(length);
        consume_budget().await;
    }
    let digests = hasher.finish();
    options.chunk = Some(METADATA_CHUNK);
    options.concurrent = 1;
    let range_reader = op.reader_options(path, options).await?;
    // Either fallback rereads and hashes the full archive again.
    let info = match metadata_bytes(&hash_reader, range_reader, digests.size).await {
        Ok(info) => info,
        Err(MetadataReadError::UnsupportedLayout) => {
            return buffered_record(op, path, metadata).await;
        }
        Err(MetadataReadError::Storage(error)) if error.kind() == ErrorKind::Unsupported => {
            // A whole-file read still works when range reads are unavailable.
            return buffered_record(op, path, metadata).await;
        }
        Err(MetadataReadError::Storage(error)) => return Err(error),
    };
    spawn_blocking(move || {
        let decoder = Decoder::new(info.reader())?;
        read_indexed_json_from_archive(digests, &mut Archive::new(decoder))
    })
    .await
    .map_err(|error| Error::new(ErrorKind::Unexpected, "metadata worker failed").set_source(error))?
    .map_err(storage_error)
}

/// Keep I/O errors retryable and send ZIP format rejections to the buffered parser.
fn zip_error(error: ZipError) -> MetadataReadError {
    match error {
        ZipError::UpstreamReadError(error) => MetadataReadError::Storage(storage_error(error)),
        _ => MetadataReadError::UnsupportedLayout,
    }
}

/// Fetch the compressed info component using bounded directory/header reads.
/// A separate range reader prevents ZIP seeks from downloading the remaining payload.
async fn metadata_bytes(
    hash_reader: &Reader,
    range_reader: Reader,
    size: u64,
) -> Result<Buffer, MetadataReadError> {
    let source = range_reader.into_futures_async_read(0..size).await?;
    let mut archive = ZipFileReader::new(source).await.map_err(zip_error)?;
    let (index, length) = {
        let entries = archive.file().entries();
        let mut names = HashSet::new();
        if entries
            .iter()
            .any(|entry| !names.insert(entry.filename().as_bytes()))
        {
            return Err(MetadataReadError::UnsupportedLayout);
        }
        // The package-streaming reader uses this naming test but does not expose a helper.
        let mut info = entries.iter().enumerate().filter(|(_, entry)| {
            entry
                .filename()
                .as_str()
                .is_ok_and(|name| name.starts_with("info-") && name.ends_with(".tar.zst"))
        });
        let (index, entry) = info.next().ok_or(MetadataReadError::UnsupportedLayout)?;
        if info.next().is_some() || entry.compression() != Compression::Stored {
            return Err(MetadataReadError::UnsupportedLayout);
        }
        (index, entry.compressed_size())
    };
    // Advance past the local header so stream_position below gives the component's data offset.
    drop(archive.reader_with_entry(index).await.map_err(zip_error)?);
    let start = archive
        .inner_mut()
        .stream_position()
        .await
        .map_err(|error| MetadataReadError::Storage(storage_error(error)))?;
    hash_reader
        .read(start..start + length)
        .await
        .map_err(MetadataReadError::Storage)
}

/// Read one immutable archive before starting CPU work on the blocking pool.
/// Finishing I/O first prevents filesystem reads from deadlocking that same pool.
async fn buffered_record(
    op: &Operator,
    path: &str,
    metadata: &RepodataFileMetadata,
) -> opendal::Result<IndexedPackageRecord> {
    let buffer = read_with_metadata_check(op, path, metadata).await?;
    spawn_blocking(move || buffered_snapshot(buffer))
        .await
        .map_err(|error| {
            Error::new(ErrorKind::Unexpected, "metadata worker failed").set_source(error)
        })?
        .map_err(storage_error)
}

/// Hash and parse an immutable buffer without copying its segments into another archive-sized allocation.
fn buffered_snapshot(buffer: Buffer) -> io::Result<IndexedPackageRecord> {
    let mut hasher = ArchiveHasher::new();
    for chunk in buffer.clone() {
        hasher.update(&chunk)?;
    }
    let mut archive = stream_conda_info(BufferCursor::new(buffer)).map_err(io::Error::other)?;
    read_indexed_json_from_archive(hasher.finish(), &mut archive)
}

/// Recover storage errors from async ZIP's `UpstreamReadError` and the nested
/// `io::Error` wrappers used by `OpenDAL`'s futures reader, preserving replacement retries.
fn storage_error(error: io::Error) -> Error {
    let mut source: &(dyn StdError + 'static) = &error;
    let kind = loop {
        if let Some(storage) = source.downcast_ref::<Error>() {
            break storage.kind();
        }
        let nested = source
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| inner as &(dyn StdError + 'static));
        match nested.or_else(|| source.source()) {
            Some(next) => source = next,
            None => break ErrorKind::Unexpected,
        }
    };
    Error::new(kind, "could not parse package metadata").set_source(error)
}

#[cfg(test)]
mod tests;
