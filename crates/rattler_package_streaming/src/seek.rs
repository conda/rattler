//! Functionality to stream parts of a `.conda` archive for objects that implement both
//! [`std::io::Read`] and [`std::io::Seek`] like a [`std::fs::File`] or a [`std::io::Cursor<T>`].

use crate::read::{stream_tar_bz2, stream_tar_zst};
use crate::ExtractError;
use rattler_conda_types::package::CondaArchiveType;
use rattler_conda_types::package::PackageFile;
use std::fs::File;
use std::io::Write;
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
use tar::Archive;
use zip::CompressionMethod;

/// A file entry in a Conda package archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFileEntry {
    /// The path of the file relative to the listing root.
    pub path: PathBuf,
    /// The uncompressed size of the file in bytes.
    pub size: u64,
}

fn stream_conda_zip_entry<'a>(
    mut archive: zip::ZipArchive<impl Read + Seek + 'a>,
    file_name: &str,
) -> Result<tar::Archive<impl Read + Sized + 'a>, ExtractError> {
    // Find the offset and size of the file in the zip.
    let (offset, size) = {
        let entry = archive.by_name(file_name)?;

        // Make sure the file is uncompressed.
        if entry.compression() != CompressionMethod::Stored {
            return Err(ExtractError::UnsupportedCompressionMethod);
        }

        (
            entry
                .data_start()
                .expect("data_start is available after reading entry"),
            entry.size(),
        )
    };

    // Seek to the position of the file
    let mut reader = archive.into_inner();
    reader.seek(SeekFrom::Start(offset))?;

    // Given the bytes in the zip archive of the file, decode it as a zst compressed tar file.
    stream_tar_zst(reader.take(size))
}

/// Stream the info section of a `.conda` package as a tar archive.
pub fn stream_conda_info<'a>(
    reader: impl Read + Seek + 'a,
) -> Result<tar::Archive<impl Read + Sized + 'a>, ExtractError> {
    let archive = zip::ZipArchive::new(reader)?;

    // Find the info entry in the archive
    let file_name = archive
        .file_names()
        .find(|file_name| file_name.starts_with("info-") && file_name.ends_with(".tar.zst"))
        .ok_or(ExtractError::MissingComponent)?
        .to_owned();

    stream_conda_zip_entry(archive, &file_name)
}

/// Stream the content section of a `.conda` package as a tar archive.
pub fn stream_conda_content<'a>(
    reader: impl Read + Seek + 'a,
) -> Result<tar::Archive<impl Read + Sized + 'a>, ExtractError> {
    let archive = zip::ZipArchive::new(reader)?;

    // Find the content entry in the archive
    let file_name = archive
        .file_names()
        .find(|file_name| file_name.starts_with("pkg-") && file_name.ends_with(".tar.zst"))
        .ok_or(ExtractError::MissingComponent)?
        .to_owned();

    stream_conda_zip_entry(archive, &file_name)
}

fn get_file_from_archive(
    archive: &mut Archive<impl Read>,
    file_name: &Path,
) -> Result<Vec<u8>, ExtractError> {
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()? == file_name {
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    Err(ExtractError::MissingComponent)
}

fn list_files_in_archive(
    archive: &mut Archive<impl Read>,
    root: Option<&Path>,
) -> Result<Vec<PackageFileEntry>, ExtractError> {
    let mut entries = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path()?.into_owned();
        let relative_path = match root {
            Some(root) => match path.strip_prefix(root) {
                Ok(relative_path) if !relative_path.as_os_str().is_empty() => {
                    relative_path.to_owned()
                }
                _ => continue,
            },
            None => path,
        };

        entries.push(PackageFileEntry {
            path: relative_path,
            size: entry.size(),
        });
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// Read a package file content from archive based on the path
pub fn read_package_file_content<'a>(
    file: impl Read + Seek + 'a,
    archive_type: CondaArchiveType,
    package_path: impl AsRef<Path>,
) -> Result<Vec<u8>, ExtractError> {
    match archive_type {
        CondaArchiveType::TarBz2 => {
            let mut archive = stream_tar_bz2(file);
            let buf = get_file_from_archive(&mut archive, package_path.as_ref())?;
            Ok(buf)
        }
        CondaArchiveType::Conda => {
            let mut info_archive = stream_conda_info(file)?;
            let buf = get_file_from_archive(&mut info_archive, package_path.as_ref())?;
            Ok(buf)
        }
    }
}

/// Read a package file from archive
/// Note: If you want to extract multiple `info/*` files then this will be slightly
///       slower than manually iterating over the archive entries with
///       custom logic as this skips over the rest of the archive
///
/// # Example
///
/// ```rust,no_run
/// use rattler_conda_types::package::AboutJson;
/// use rattler_package_streaming::seek::read_package_file;
///
/// let about_json = read_package_file::<AboutJson>("conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.conda").unwrap();
/// ```
pub fn read_package_file<P: PackageFile>(path: impl AsRef<Path>) -> Result<P, ExtractError> {
    // stream extract the file from a package
    let file = File::open(&path)?;
    let content = read_package_file_content(
        &file,
        CondaArchiveType::try_from(&path).ok_or(ExtractError::UnsupportedArchiveType)?,
        P::package_path(),
    )?;

    P::from_slice(&content)
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
}

/// Get a [`PackageFile`] from temporary archive and extract it to a writer
pub fn extract_package_file<'a, P: PackageFile>(
    reader: impl Read + Seek + 'a,
    location: &Path,
    writer: &mut impl Write,
) -> Result<(), ExtractError> {
    let content = read_package_file_content(
        reader,
        CondaArchiveType::try_from(location).ok_or(ExtractError::UnsupportedArchiveType)?,
        P::package_path(),
    )?;

    writer.write_all(&content)?;

    writer.flush()?;

    Ok(())
}

/// List all files in the `info/` section of a local package archive.
pub fn list_info_files(path: impl AsRef<Path>) -> Result<Vec<PackageFileEntry>, ExtractError> {
    let path = path.as_ref();
    let file = File::open(path)?;
    match CondaArchiveType::try_from(path).ok_or(ExtractError::UnsupportedArchiveType)? {
        CondaArchiveType::TarBz2 => {
            let mut archive = stream_tar_bz2(file);
            list_files_in_archive(&mut archive, Some(Path::new("info")))
        }
        CondaArchiveType::Conda => {
            let mut archive = stream_conda_info(file)?;
            list_files_in_archive(&mut archive, Some(Path::new("info")))
        }
    }
}
