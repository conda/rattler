//! Functions that enable extracting or streaming a Conda package for objects
//! that implement the [`tokio::io::AsyncRead`] + [`tokio::io::AsyncSeek`] traits.

use std::path::Path;

use async_zip::base::read::seek::ZipFileReader;
use tokio::io::{AsyncRead, AsyncSeek};
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};

use crate::ExtractError;

use super::shared::{extract_tar_zst_entry, DEFAULT_BUF_SIZE};

/// Extracts the contents of a `.conda` package archive using the seek-based API.
/// This is more efficient than streaming when the entire file is available (e.g., from disk or memory).
///
/// This function only performs extraction and does NOT compute hashes or track size.
/// Use this when you've already computed hashes separately or don't need them.
pub async fn extract_conda(
    reader: impl AsyncRead + AsyncSeek + Send + Unpin + 'static,
    destination: &Path,
) -> Result<(), ExtractError> {
    // Ensure the destination directory exists
    tokio::fs::create_dir_all(destination)
        .await
        .map_err(ExtractError::CouldNotCreateDestination)?;

    // Clone destination for the async block
    let destination = destination.to_owned();

    // Convert to futures traits for async_zip (which uses futures traits)
    let mut compat_reader = reader.compat();
    let mut buf_reader =
        futures::io::BufReader::with_capacity(DEFAULT_BUF_SIZE, &mut compat_reader);

    // Create a ZIP reader using the seek API
    let mut zip_reader = ZipFileReader::new(&mut buf_reader)
        .await
        .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?;

    // Process each ZIP entry
    let num_entries = zip_reader.file().entries().len();
    for index in 0..num_entries {
        let entry = zip_reader.file().entries().get(index).ok_or_else(|| {
            ExtractError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "entry not found",
            ))
        })?;

        let filename = entry.filename().as_str().map_err(|e| {
            ExtractError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;

        // Only extract .tar.zst files
        if filename.ends_with(".tar.zst") {
            let entry_reader = zip_reader
                .reader_with_entry(index)
                .await
                .map_err(|e| ExtractError::IoError(std::io::Error::other(e)))?;

            // Convert from futures traits to tokio traits
            let mut compat_entry = entry_reader.compat();
            extract_tar_zst_entry(&mut compat_entry, &destination).await?;
        }
    }

    Ok(())
}
