//! Functions that enable extracting or streaming a Conda package for objects
//! that implement the [`tokio::io::AsyncRead`] trait.

use std::path::Path;

use async_compression::tokio::bufread::BzDecoder;
use async_spooled_tempfile::SpooledTempFile;
use async_zip::base::read::stream::ZipFileReader;
use tokio::io::{AsyncRead, AsyncSeekExt};
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};

use crate::{read::SizeCountingReader, ExtractError, ExtractResult};

use super::shared::{extract_tar_zst_entry, unpack_tar_archive, DEFAULT_BUF_SIZE};

/// Extracts the contents a `.tar.bz2` package archive using fully async implementation.
pub async fn extract_tar_bz2(
    reader: impl AsyncRead + Send + Unpin + 'static,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Ensure the destination directory exists
    tokio::fs::create_dir_all(destination)
        .await
        .map_err(ExtractError::CouldNotCreateDestination)?;

    // Clone destination for the async block
    let destination = destination.to_owned();

    // Wrap the reading in additional readers that will compute the hashes while extracting
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);
    let mut size_reader = SizeCountingReader::new(&mut md5_reader);

    // Create a buffered reader for better performance
    let buf_reader = tokio::io::BufReader::with_capacity(DEFAULT_BUF_SIZE, &mut size_reader);

    // Decompress bzip2 asynchronously
    let decoder = BzDecoder::new(buf_reader);

    // Build archive with optimized settings for faster extraction:
    // - Skip mtime preservation to avoid extra syscalls
    // - Skip automatic permission handling (we'll set executable bits manually)
    // - Skip extended attributes for better performance
    let archive = tokio_tar::ArchiveBuilder::new(decoder)
        .set_preserve_mtime(true)
        .set_preserve_permissions(false)
        .set_unpack_xattrs(false)
        .build();

    // Unpack entries manually, preserving only executable bits on Unix
    unpack_tar_archive(archive, &destination).await?;

    // Read the file to the end to make sure the hash is properly computed
    tokio::io::copy(&mut size_reader, &mut tokio::io::sink())
        .await
        .map_err(ExtractError::IoError)?;

    // Get the size and hashes
    let (_, total_size) = size_reader.finalize();
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    // Validate that we actually read some data from the stream.
    // If total_size is 0, it likely means the stream was truncated or the bzip2
    // decompressor silently failed without detecting an incomplete stream.
    if total_size == 0 {
        return Err(ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "no data was read from the package stream - the stream may have been truncated",
        )));
    }

    Ok(ExtractResult {
        sha256,
        md5,
        total_size,
    })
}

/// Extracts the contents of a `.conda` package archive using fully async implementation.
/// This will perform on-the-fly decompression by streaming the reader.
pub async fn extract_conda(
    reader: impl AsyncRead + Send + Unpin + 'static,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Ensure the destination directory exists
    tokio::fs::create_dir_all(destination)
        .await
        .map_err(ExtractError::CouldNotCreateDestination)?;

    // Clone destination for the async block
    let destination = destination.to_owned();

    // Wrap the reading in additional readers that will compute the hashes while extracting
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);
    let mut size_reader = SizeCountingReader::new(&mut md5_reader);

    // Convert to futures traits and create a buffered reader (async_zip uses futures traits)
    let compat_reader = (&mut size_reader).compat();
    let mut buf_reader = futures::io::BufReader::with_capacity(DEFAULT_BUF_SIZE, compat_reader);

    // Create a ZIP reader for streaming
    let mut zip_reader = ZipFileReader::new(&mut buf_reader);

    // Process each ZIP entry
    while let Some(mut entry) = zip_reader
        .next_with_entry()
        .await
        .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?
    {
        let filename = entry.reader().entry().filename().as_str().map_err(|e| {
            ExtractError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;

        // Only extract .tar.zst files
        if filename.ends_with(".tar.zst") {
            // Get a reader for the entry and convert from futures traits to tokio traits
            let mut compat_entry = entry.reader_mut().compat();
            extract_tar_zst_entry(&mut compat_entry, &destination).await?;
        }

        // Skip to the next entry (required by async_zip API)
        (.., zip_reader) = entry
            .skip()
            .await
            .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?;
    }

    // Read any remaining data to ensure hash is properly computed
    // Use futures copy since we're already in futures ecosystem
    futures::io::copy(&mut buf_reader, &mut futures::io::sink())
        .await
        .map_err(ExtractError::IoError)?;

    // Get the size and hashes
    let (_, total_size) = size_reader.finalize();
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    // Validate that we actually read some data from the stream
    if total_size == 0 {
        return Err(ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "no data was read from the package stream - the stream may have been truncated",
        )));
    }

    Ok(ExtractResult {
        sha256,
        md5,
        total_size,
    })
}

/// Extracts the contents of a .conda package archive by fully reading the
/// stream and then decompressing. This is a fallback method for when streaming fails.
///
/// This implementation uses a `SpooledTempFile` (5MB in-memory threshold) to buffer
/// the package data, then uses the seek-based ZIP API for efficient extraction.
pub async fn extract_conda_via_buffering(
    reader: impl AsyncRead + Send + Unpin + 'static,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Delete destination first if it exists, as this method is usually used as a fallback
    if tokio::fs::try_exists(destination)
        .await
        .map_err(ExtractError::IoError)?
    {
        tokio::fs::remove_dir_all(destination)
            .await
            .map_err(ExtractError::CouldNotCreateDestination)?;
    }

    // Ensure the destination directory exists
    tokio::fs::create_dir_all(destination)
        .await
        .map_err(ExtractError::CouldNotCreateDestination)?;

    // Clone destination for the async block
    let destination = destination.to_owned();

    // Wrap the reading in additional readers that will compute the hashes while extracting
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);
    let mut size_reader = SizeCountingReader::new(&mut md5_reader);

    // Create a SpooledTempFile (uses memory up to 5MB, then switches to disk)
    let mut spooled_file = SpooledTempFile::new(5 * 1024 * 1024);

    // Copy from reader to spooled file while computing hashes
    tokio::io::copy(&mut size_reader, &mut spooled_file)
        .await
        .map_err(ExtractError::IoError)?;

    // Get the size and hashes now that we've read everything
    let (_, total_size) = size_reader.finalize();
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    // Validate that we actually read some data from the stream
    if total_size == 0 {
        return Err(ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "no data was read from the package stream - the stream may have been truncated",
        )));
    }

    // Rewind the spooled file to the beginning
    spooled_file.rewind().await.map_err(ExtractError::IoError)?;

    // Use the seek-based extraction (doesn't recompute hashes, we already have them)
    crate::tokio::async_seek::extract_conda(spooled_file, &destination).await?;

    Ok(ExtractResult {
        sha256,
        md5,
        total_size,
    })
}
