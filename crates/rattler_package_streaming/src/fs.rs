//! Functions to extracting or stream a Conda package from a file on disk.

use crate::{read, seek};
use crate::{ExtractError, ExtractResult};
use rattler_conda_types::package::ArchiveType;
use std::fs::File;
use std::io::BufReader;
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
    crate::read::extract_conda(file, destination)
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

/// Extracts a specified directory from .tar.bz2 into a destination folder.
///
/// ```rust,no_run
/// # use std::path::Path;
/// use rattler_package_streaming::fs::extract_directory_from_tar_bz2;
/// let _ = extract_directory_from_tar_bz2(Path::new("archive/location"), Path::new("directory_to_extract"), Path::new("destination/directory"))
/// ```
pub fn extract_directory_from_tar_bz2(
    archive_path: &Path,
    directory_to_extract: &Path,
    dest_directory: &Path,
) -> Result<(), std::io::Error> {
    let reader = std::fs::File::open(archive_path)?;
    let mut archive = read::stream_tar_bz2(reader);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;

        if let Ok(stripped_path) = path.strip_prefix(directory_to_extract) {
            let dest_file = dest_directory.join(stripped_path);
            if let Some(parent_folder) = dest_file.parent() {
                if !parent_folder.exists() {
                    std::fs::create_dir_all(parent_folder)?;
                }
            }
            let mut dest_file = std::fs::File::create(dest_file)?;
            std::io::copy(&mut entry, &mut dest_file)?;
        }
    }
    Ok(())
}

/// Extracts a specified directory from .conda archive into a destination folder.
///
/// ```rust,no_run
/// # use std::path::Path;
/// use rattler_package_streaming::fs::extract_directory_from_tar_bz2;
/// let _ = extract_directory_from_conda(Path::new("archive/location"), Path::new("directory_to_extract"), Path::new("destination/directory"))
/// ```
pub fn extract_directory_from_conda(
    archive_path: &Path,
    directory_to_extract: &Path,
    dest_directory: &Path,
) -> Result<(), std::io::Error> {
    let reader = std::fs::File::open(archive_path)?;
    let buf_reader = BufReader::new(reader);

    let mut archive = if directory_to_extract.starts_with("info") {
        seek::stream_conda_info(buf_reader).expect("Could not open conda file")
    } else {
        todo!("Not implemented yet");
    };

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if let Ok(stripped_path) = path.strip_prefix(directory_to_extract) {
            let dest_file = dest_directory.join(stripped_path);
            if let Some(parent_folder) = dest_file.parent() {
                if !parent_folder.exists() {
                    std::fs::create_dir_all(parent_folder)?;
                }
            }
            let mut dest_file = std::fs::File::create(dest_file)?;
            std::io::copy(&mut entry, &mut dest_file)?;
        }
    }
    Ok(())
}
