//! Functions to extracting or stream a Conda package from a file on disk.

use crate::{ExtractError, ExtractResult};
use rattler_conda_types::package::CondaArchiveType;
use std::path::Path;

/// Extracts the contents a `.tar.bz2` package archive at the specified path to a directory.
///
/// If `cas_store` is provided, file contents are stored in the CAS and hardlinked
/// to the destination for deduplication.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use rattler_package_streaming::tokio::fs::extract_tar_bz2;
/// let _ = extract_tar_bz2(
///     Path::new("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2"),
///     Path::new("/tmp"),
///     None)
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_tar_bz2(
    archive: &Path,
    destination: &Path,
    cas_store: Option<&Path>,
) -> Result<ExtractResult, ExtractError> {
    // Open the file for reading using async I/O
    let file = tokio::fs::File::open(archive)
        .await
        .map_err(ExtractError::IoError)?;

    crate::tokio::async_read::extract_tar_bz2(file, destination, cas_store).await
}

/// Extracts the contents a `.conda` package archive at the specified path to a directory.
///
/// If `cas_store` is provided, file contents are stored in the CAS and hardlinked
/// to the destination for deduplication.
///
/// ```rust,no_run
/// # use std::path::Path;
/// # #[tokio::main]
/// # async fn main() {
/// use rattler_package_streaming::tokio::fs::extract_conda;
/// let _ = extract_conda(
///     Path::new("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.conda"),
///     Path::new("/tmp"),
///     None)
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_conda(
    archive: &Path,
    destination: &Path,
    cas_store: Option<&Path>,
) -> Result<ExtractResult, ExtractError> {
    // Open the file for reading using async I/O
    let file = tokio::fs::File::open(archive)
        .await
        .map_err(ExtractError::IoError)?;

    crate::tokio::async_read::extract_conda(file, destination, cas_store).await
}

/// Extracts the contents a package archive at the specified path to a directory. The type of
/// package is determined based on the file extension of the archive path.
///
/// If `cas_store` is provided, file contents are stored in the CAS and hardlinked
/// to the destination for deduplication.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use rattler_package_streaming::tokio::fs::extract;
/// let _ = extract(
///     Path::new("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.conda"),
///     Path::new("/tmp"),
///     None)
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract(
    archive: &Path,
    destination: &Path,
    cas_store: Option<&Path>,
) -> Result<ExtractResult, ExtractError> {
    match CondaArchiveType::try_from(archive).ok_or(ExtractError::UnsupportedArchiveType)? {
        CondaArchiveType::TarBz2 => extract_tar_bz2(archive, destination, cas_store).await,
        CondaArchiveType::Conda => extract_conda(archive, destination, cas_store).await,
    }
}
