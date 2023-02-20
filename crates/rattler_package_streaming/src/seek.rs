//! Functionality to stream parts of a `.conda` archive for objects that implement both
//! [`std::io::Read`] and [`std::io::Seek`] like a [`std::fs::File`] or a [`std::io::Cursor<T>`].

use crate::read::stream_tar_zst;
use crate::ExtractError;
use std::io::{Read, Seek, SeekFrom};
use zip::CompressionMethod;

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

        (entry.data_start(), entry.size())
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
