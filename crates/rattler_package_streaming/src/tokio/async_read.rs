//! Functions that enable extracting or streaming a Conda package for objects
//! that implement the [`tokio::io::AsyncRead`] trait.

use std::{io::Read, path::Path};

use tokio::io::AsyncRead;
use tokio_util::io::SyncIoBridge;

use crate::{ExtractError, ExtractResult};

/// Extracts the contents a `.tar.bz2` package archive.
pub async fn extract_tar_bz2(
    reader: impl AsyncRead + Send + 'static,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Create a async -> sync bridge
    let reader = SyncIoBridge::new(Box::pin(reader));

    // Spawn a block task to perform the extraction
    let destination = destination.to_owned();
    match tokio::task::spawn_blocking(move || crate::read::extract_tar_bz2(reader, &destination))
        .await
    {
        Ok(result) => result,
        Err(err) => {
            if let Ok(reason) = err.try_into_panic() {
                std::panic::resume_unwind(reason);
            }
            Err(ExtractError::Cancelled)
        }
    }
}

/// Extracts the contents of a `.conda` package archive.
/// This will perform on-the-fly decompression by streaming the reader.
pub async fn extract_conda(
    reader: impl AsyncRead + Send + 'static,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    extract_conda_internal(
        reader,
        destination,
        crate::read::extract_conda_via_streaming,
    )
    .await
}

/// Extracts the contents of a .conda package archive by fully reading the
/// stream and then decompressing
pub async fn extract_conda_via_buffering(
    reader: impl AsyncRead + Send + 'static,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    extract_conda_internal(
        reader,
        destination,
        crate::read::extract_conda_via_buffering,
    )
    .await
}

/// Extracts the contents of a `.conda` package archive using the provided
/// extraction function
async fn extract_conda_internal(
    reader: impl AsyncRead + Send + 'static,
    destination: &Path,
    extract_fn: fn(Box<dyn Read>, &Path) -> Result<ExtractResult, ExtractError>,
) -> Result<ExtractResult, ExtractError> {
    // Create a async -> sync bridge
    let reader = SyncIoBridge::new(Box::pin(reader));

    // Spawn a block task to perform the extraction
    let destination = destination.to_owned();
    tokio::task::spawn_blocking(move || {
        let reader: Box<dyn Read> = Box::new(reader);
        extract_fn(reader, &destination)
    })
    .await
    .unwrap_or_else(|err| {
        if let Ok(reason) = err.try_into_panic() {
            std::panic::resume_unwind(reason);
        }
        Err(ExtractError::Cancelled)
    })
}
