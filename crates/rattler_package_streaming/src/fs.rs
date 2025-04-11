//! Functions to extracting or stream a Conda package from a file on disk.

use crate::{ExtractError, ExtractResult};
use rattler_conda_types::package::ArchiveType;
use std::fs::File;
use std::path::Path;

/// Extracts the contents a `.tar.bz2` package archive at the specified path to a directory.
///
/// ```rust,no_run
/// # use std::path::Path;
/// use rattler_package_streaming::fs::extract_tar_bz2;
/// let _ = extract_tar_bz2(
///     Path::new("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2"),
///     Path::new("/tmp"))
///     .unwrap();
/// ```
pub fn extract_tar_bz2(archive: &Path, destination: &Path) -> Result<ExtractResult, ExtractError> {
    let file = File::open(archive)?;
    crate::read::extract_tar_bz2(file, destination)
}

/// Extracts the contents a `.conda` package archive at the specified path to a directory.
///
/// ```rust,no_run
/// # use std::path::Path;
/// use rattler_package_streaming::fs::extract_conda;
/// let _ = extract_conda(
///     Path::new("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.conda"),
///     Path::new("/tmp"))
///     .unwrap();
/// ```
pub fn extract_conda(archive: &Path, destination: &Path) -> Result<ExtractResult, ExtractError> {
    let file = File::open(archive)?;
    crate::read::extract_conda_via_streaming(file, destination)
}

/// Extracts the contents a package archive at the specified path to a directory. The type of
/// package is determined based on the file extension of the archive path.
///
/// ```rust,no_run
/// # use std::path::Path;
/// use rattler_package_streaming::fs::extract;
/// let _ = extract(
///     Path::new("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.conda"),
///     Path::new("/tmp"))
///     .unwrap();
/// ```
pub fn extract(archive: &Path, destination: &Path) -> Result<ExtractResult, ExtractError> {
    match ArchiveType::try_from(archive).ok_or(ExtractError::UnsupportedArchiveType)? {
        ArchiveType::TarBz2 => extract_tar_bz2(archive, destination),
        ArchiveType::Conda => extract_conda(archive, destination),
    }
}
