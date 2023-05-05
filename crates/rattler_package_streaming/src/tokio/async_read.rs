//! Functions that enable extracting or streaming a Conda package for objects that implement the
//! [`tokio::io::AsyncRead`] trait.

use crate::{ExtractError, ExtractResult};
use std::path::Path;
use tokio::io::AsyncRead;
use tokio_util::io::SyncIoBridge;

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
pub async fn extract_conda(
    reader: impl AsyncRead + Send + 'static,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Create a async -> sync bridge
    let reader = SyncIoBridge::new(Box::pin(reader));

    // Spawn a block task to perform the extraction
    let destination = destination.to_owned();
    match tokio::task::spawn_blocking(move || crate::read::extract_conda(reader, &destination))
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
