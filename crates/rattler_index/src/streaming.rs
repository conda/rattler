use std::io;

use async_zip::{Compression, base::read::seek::ZipFileReader, error::ZipError};
use bytes::Buf;
use futures::{AsyncBufReadExt, AsyncSeekExt};
use opendal::{Buffer, Error, ErrorKind, Operator, Reader, options::ReaderOptions};
use rattler_digest::{Md5, Sha256};
use sha2::Digest;

use crate::{IndexedPackageRecord, RepodataFileMetadata};

// Bound ZIP-directory/header requests; the archive hash uses one continuous GET.
const METADATA_CHUNK: usize = 64 * 1024;

/// Hash and read metadata from the same guarded object version. Only in-memory
/// hashing/parsing runs on the blocking pool, so storage I/O remains cancellable.
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
    let (mut md5, mut sha256, mut size) = (Md5::new(), Sha256::new(), 0u64);
    loop {
        let chunk = source.fill_buf().await.map_err(storage_error)?;
        if chunk.is_empty() {
            break;
        }
        // Keep hashing work bounded per budget unit, even for whole-archive buffers.
        let chunk = &chunk[..chunk.len().min(8 * 1024)];
        md5.update(chunk);
        sha256.update(chunk);
        let length = chunk.len();
        size += length as u64;
        source.consume_unpin(length);
        tokio::task::consume_budget().await;
    }

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
    let mut indexed = tokio::task::spawn_blocking(move || {
        let decoder = zstd::stream::read::Decoder::new(info.reader())?;
        crate::read_indexed_json_from_archive(None, &mut tar::Archive::new(decoder))
    })
    .await
    .map_err(|error| Error::new(ErrorKind::Unexpected, "metadata worker failed").set_source(error))?
    .map_err(storage_error)?;
    indexed.record.md5 = Some(md5.finalize());
    indexed.record.sha256 = Some(sha256.finalize());
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

fn zip_error(error: ZipError) -> Error {
    match error {
        ZipError::UpstreamReadError(error) => storage_error(error),
        // A format rejection must use the compatible snapshot parser.
        error => Error::new(ErrorKind::Unsupported, "unsupported ZIP layout").set_source(error),
    }
}

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

async fn buffered_record(
    op: &Operator,
    path: &str,
    metadata: &RepodataFileMetadata,
) -> opendal::Result<IndexedPackageRecord> {
    let buffer = crate::utils::read_with_metadata_check(op, path, metadata).await?;
    tokio::task::spawn_blocking(move || buffered_snapshot(buffer))
        .await
        .map_err(|error| {
            Error::new(ErrorKind::Unexpected, "metadata worker failed").set_source(error)
        })?
        .map_err(storage_error)
}

fn buffered_snapshot(buffer: Buffer) -> io::Result<IndexedPackageRecord> {
    // Keep one immutable snapshot without coalescing segmented storage into a
    // second archive-sized allocation, including on timestamp-only filesystems.
    let (mut md5, mut sha256) = (Md5::new(), Sha256::new());
    for chunk in buffer.clone() {
        md5.update(&chunk);
        sha256.update(&chunk);
    }
    let size = buffer.len() as u64;
    let mut archive =
        rattler_package_streaming::seek::stream_conda_info(opendal::BufferCursor::new(buffer))
            .map_err(io::Error::other)?;
    let mut indexed = crate::read_indexed_json_from_archive(None, &mut archive)?;
    indexed.record.md5 = Some(md5.finalize());
    indexed.record.sha256 = Some(sha256.finalize());
    indexed.record.size = Some(size);
    Ok(indexed)
}

// Later range requests may detect replacement; preserve their storage error
// kind through I/O wrappers so the retry loop rereads the whole record.
fn storage_error(error: io::Error) -> Error {
    let mut source: &(dyn std::error::Error + 'static) = &error;
    let kind = loop {
        if let Some(storage) = source.downcast_ref::<Error>() {
            break storage.kind();
        }
        let nested = source
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| inner as &(dyn std::error::Error + 'static));
        match nested.or_else(|| source.source()) {
            Some(next) => source = next,
            None => break ErrorKind::Unexpected,
        }
    };
    Error::new(kind, "could not parse package metadata").set_source(error)
}

#[cfg(test)]
mod tests;
