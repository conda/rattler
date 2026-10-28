//! Functions that enable extracting or streaming a Conda package for objects
//! that implement the [`tokio::io::AsyncRead`] + [`tokio::io::AsyncSeek`] traits.

use std::path::Path;

use tokio::io::{AsyncRead, AsyncSeek, AsyncSeekExt};
use zip::result::ZipError;

use crate::ExtractError;
use crate::read::DATA_DESCRIPTOR_ERROR_MESSAGE;

/// Extracts the contents of a `.conda` package archive from a seekable reader.
///
/// The archive is extracted while it is read, on the same code path as
/// [`super::async_read::extract_conda`]. Archives that use zip data
/// descriptors cannot be extracted that way; the reader is then rewound and
/// the archive is buffered before extraction.
///
/// This function only performs extraction and does NOT compute hashes or track size.
/// Use this when you've already computed hashes separately or don't need them.
pub async fn extract_conda(
    mut reader: impl AsyncRead + AsyncSeek + Send + Unpin + 'static,
    destination: &Path,
) -> Result<(), ExtractError> {
    match super::async_read::extract_conda(&mut reader, destination).await {
        Ok(_) => Ok(()),
        Err(ExtractError::ZipError(ZipError::UnsupportedArchive(message)))
            if message.contains(DATA_DESCRIPTOR_ERROR_MESSAGE) =>
        {
            reader.rewind().await.map_err(ExtractError::IoError)?;
            super::async_read::extract_conda_via_buffering(reader, destination)
                .await
                .map(|_| ())
        }
        Err(err) => Err(err),
    }
}
