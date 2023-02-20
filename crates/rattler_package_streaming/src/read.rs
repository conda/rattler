//! Functions that enable extracting or streaming a Conda package for objects that implement the
//! [`std::io::Read`] trait.

use super::ExtractError;
use std::ffi::OsStr;
use std::{io::Read, path::Path};
use zip::read::read_zipfile_from_stream;

/// Returns the `.tar.bz2` as a decompressed `tar::Archive`. The `tar::Archive` can be used to
/// extract the files from it, or perform introspection.
pub fn stream_tar_bz2(reader: impl Read) -> tar::Archive<impl Read + Sized> {
    tar::Archive::new(bzip2::read::BzDecoder::new(reader))
}

/// Returns the `.tar.zst` as a decompressed `tar` archive. The `tar::Archive` can be used to
/// extract the files from it, or perform introspection.
pub(crate) fn stream_tar_zst(
    reader: impl Read,
) -> Result<tar::Archive<impl Read + Sized>, ExtractError> {
    Ok(tar::Archive::new(zstd::stream::read::Decoder::new(reader)?))
}

/// Extracts the contents a `.tar.bz2` package archive.
pub fn extract_tar_bz2(reader: impl Read, destination: &Path) -> Result<(), ExtractError> {
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;
    stream_tar_bz2(reader).unpack(destination)?;
    Ok(())
}

/// Extracts the contents of a `.conda` package archive.
pub fn extract_conda(mut reader: impl Read, destination: &Path) -> Result<(), ExtractError> {
    // Construct the destination path if it doesnt exist yet
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    // Iterate over all entries in the zip-file and extract them one-by-one
    while let Some(file) = read_zipfile_from_stream(&mut reader)? {
        if file
            .mangled_name()
            .file_name()
            .map(OsStr::to_string_lossy)
            .map_or(false, |file_name| file_name.ends_with(".tar.zst"))
        {
            stream_tar_zst(file)?.unpack(destination)?;
        }
    }

    Ok(())
}
