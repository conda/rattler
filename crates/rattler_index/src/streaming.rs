//! Read `.conda` records without retaining the payload when object identity is available.
//! Unversioned storage and unsupported ZIP layouts use a single buffered snapshot.

use std::{
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
    IndexedPackageRecord, RepodataFileMetadata, read_indexed_json_from_archive,
    utils::read_with_metadata_check,
};

// Bound ZIP-directory/header requests; the archive hash uses one continuous GET.
const METADATA_CHUNK: usize = 64 * 1024;

/// Return metadata and whole-archive digests suitable for a repodata record.
///
/// With an `ETag`, one sequential read hashes the archive and bounded range reads
/// fetch its info component. Every read uses the same version condition; callers
/// must retry the entire operation on `ConditionNotMatch`. If conditions are
/// disabled, a final stat detects replacement before the record can be cached.
///
/// Storage without `ETags` uses one immutable snapshot because timestamps cannot
/// establish consistency across separate reads. ZIP layouts rejected by the
/// range parser also use that compatibility path. Storage I/O remains async and
/// cancellable; synchronous parsing and buffered hashing use the blocking pool.
pub(crate) async fn read_package(
    op: &Operator,
    path: &str,
    metadata: &RepodataFileMetadata,
) -> opendal::Result<IndexedPackageRecord> {
    // Timestamps can miss same-size replacements within their resolution.
    // Without an ETag, preserve the original single-read snapshot.
    if metadata.etag.is_none() {
        return buffered_record(op, path, metadata).await;
    }
    let mut options = ReaderOptions::default();
    if metadata.precondition_checks.is_enabled() {
        options.if_match = metadata.etag.clone();
    }
    let reader = op.reader_options(path, options.clone()).await?;
    let mut source = reader.clone().into_futures_async_read(..).await?;
    let mut hashes = HashingWriter::<_, Md5>::new(HashingWriter::<_, Sha256>::new(io::sink()));
    let mut size = 0u64;
    loop {
        let chunk = source.fill_buf().await.map_err(storage_error)?;
        if chunk.is_empty() {
            break;
        }
        // Keep hashing work bounded per budget unit, even for whole-archive buffers.
        let chunk = &chunk[..chunk.len().min(8 * 1024)];
        hashes.write_all(chunk).map_err(storage_error)?;
        let length = chunk.len();
        size += length as u64;
        source.consume_unpin(length);
        consume_budget().await;
    }
    let (sha256, md5) = hashes.finalize();
    let (_, sha256) = sha256.finalize();

    options.chunk = Some(METADATA_CHUNK);
    options.concurrent = 1;
    let directory = op.reader_options(path, options).await?;
    let info = match metadata_bytes(&reader, directory, size).await {
        Ok(info) => info,
        // The legacy parser also accepts ZIP layouts rejected by async ZIP.
        Err(error) if error.kind() == ErrorKind::Unsupported => {
            return buffered_record(op, path, metadata).await;
        }
        Err(error) => return Err(error),
    };
    let mut indexed = spawn_blocking(move || {
        let decoder = Decoder::new(info.reader())?;
        read_indexed_json_from_archive(None, &mut Archive::new(decoder))
    })
    .await
    .map_err(|error| Error::new(ErrorKind::Unexpected, "metadata worker failed").set_source(error))?
    .map_err(storage_error)?;
    indexed.record.md5 = Some(md5);
    indexed.record.sha256 = Some(sha256);
    indexed.record.size = Some(size);

    if !metadata.precondition_checks.is_enabled() {
        // The retry loop already captured this ETag before hashing.
        let after = op.stat(path).await?;
        if metadata.etag.as_deref() != after.etag() || after.content_length() != size {
            return Err(Error::new(
                ErrorKind::ConditionNotMatch,
                "archive changed while indexing",
            ));
        }
    }
    Ok(indexed)
}

/// Distinguish unsupported ZIP layouts from storage failures so only format
/// incompatibility falls back to buffering; replacement errors must reach retries.
fn zip_error(error: ZipError) -> Error {
    match error {
        ZipError::UpstreamReadError(error) => storage_error(error),
        // A format rejection must use the compatible snapshot parser.
        error => Error::new(ErrorKind::Unsupported, "unsupported ZIP layout").set_source(error),
    }
}

/// Locate the info component through the central directory and fetch only its
/// compressed bytes. A separately chunked reader keeps ZIP seeks from requesting
/// the rest of a large payload; the hash reader retains its one sequential GET.
async fn metadata_bytes(reader: &Reader, directory: Reader, size: u64) -> opendal::Result<Buffer> {
    let source = directory.into_futures_async_read(0..size).await?;
    let mut archive = ZipFileReader::new(source).await.map_err(zip_error)?;
    let entries = archive.file().entries();
    let entry = entries
        .iter()
        .find(|entry| {
            entry
                .filename()
                .as_str()
                .is_ok_and(|name| name.starts_with("info-") && name.ends_with(".tar.zst"))
        })
        .ok_or_else(|| zip_error(ZipError::EntryIndexOutOfBounds))?;
    // The legacy ZIP reader keeps the last duplicate of the first matching name.
    let (index, entry) = entries
        .iter()
        .enumerate()
        .rfind(|(_, candidate)| candidate.filename().as_bytes() == entry.filename().as_bytes())
        .expect("the selected entry is present");
    if entry.compression() != Compression::Stored {
        return Err(zip_error(ZipError::FeatureNotSupported(
            "compressed .conda ZIP entries",
        )));
    }
    let length = entry.compressed_size();
    // The ZIP parser validates bounds and locates data using the local header;
    // its extra-field length may differ from that in the central directory.
    drop(archive.reader_with_entry(index).await.map_err(zip_error)?);
    let start = archive
        .inner_mut()
        .stream_position()
        .await
        .map_err(storage_error)?;
    // Request exactly the metadata component, rather than offset..EOF.
    reader.read(start..start + length).await
}

/// Preserve the legacy single-snapshot guarantee when separate guarded reads
/// are unavailable. Finish I/O before spawning CPU work to avoid blocking-pool
/// deadlocks on filesystem backends that also use that pool.
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

/// Hash and parse one immutable buffer without coalescing its segments into
/// another archive-sized allocation. The seek parser preserves legacy ZIP support.
fn buffered_snapshot(buffer: Buffer) -> io::Result<IndexedPackageRecord> {
    let mut hashes = HashingWriter::<_, Md5>::new(HashingWriter::<_, Sha256>::new(io::sink()));
    for chunk in buffer.clone() {
        hashes.write_all(&chunk)?;
    }
    let (sha256, md5) = hashes.finalize();
    let (_, sha256) = sha256.finalize();
    let size = buffer.len() as u64;
    let mut archive = stream_conda_info(BufferCursor::new(buffer)).map_err(io::Error::other)?;
    let mut indexed = read_indexed_json_from_archive(None, &mut archive)?;
    indexed.record.md5 = Some(md5);
    indexed.record.sha256 = Some(sha256);
    indexed.record.size = Some(size);
    Ok(indexed)
}

/// Preserve storage error kinds through ZIP/I/O wrappers so a replacement during
/// a later range request retries the whole record instead of becoming a parse error.
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
