//! Functions that enable extracting or streaming a Conda package from local
//! files in an async context. Extraction runs on a blocking worker thread
//! with plain file I/O, which is the fastest path for a file that is already
//! on disk.

use crate::{ExtractError, ExtractResult};
use rattler_conda_types::package::CondaArchiveType;
use std::path::Path;

/// Runs a blocking extractor on a worker thread.
async fn spawn_extract(
    archive: &Path,
    destination: &Path,
    extract: fn(&Path, &Path) -> Result<ExtractResult, ExtractError>,
) -> Result<ExtractResult, ExtractError> {
    let archive = archive.to_owned();
    let destination = destination.to_owned();
    match tokio::task::spawn_blocking(move || extract(&archive, &destination)).await {
        Ok(result) => result,
        Err(err) => {
            if let Ok(reason) = err.try_into_panic() {
                std::panic::resume_unwind(reason);
            }
            Err(ExtractError::Cancelled)
        }
    }
}

/// Extracts the contents a `.tar.bz2` package archive at the specified path to a directory.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use rattler_package_streaming::tokio::fs::extract_tar_bz2;
/// let _ = extract_tar_bz2(
///     Path::new("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2"),
///     Path::new("/tmp"))
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_tar_bz2(
    archive: &Path,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    spawn_extract(archive, destination, crate::fs::extract_tar_bz2).await
}

/// Extracts the contents a `.conda` package archive at the specified path to a directory.
///
/// ```rust,no_run
/// # use std::path::Path;
/// # #[tokio::main]
/// # async fn main() {
/// use rattler_package_streaming::tokio::fs::extract_conda;
/// let _ = extract_conda(
///     Path::new("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.conda"),
///     Path::new("/tmp"))
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_conda(
    archive: &Path,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    spawn_extract(archive, destination, crate::fs::extract_conda).await
}

/// Extracts the contents a package archive at the specified path to a directory. The type of
/// package is determined based on the file extension of the archive path.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use rattler_package_streaming::tokio::fs::extract;
/// let _ = extract(
///     Path::new("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.conda"),
///     Path::new("/tmp"))
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract(archive: &Path, destination: &Path) -> Result<ExtractResult, ExtractError> {
    match CondaArchiveType::try_from(archive).ok_or(ExtractError::UnsupportedArchiveType)? {
        CondaArchiveType::TarBz2 => extract_tar_bz2(archive, destination).await,
        CondaArchiveType::Conda => extract_conda(archive, destination).await,
    }
}
