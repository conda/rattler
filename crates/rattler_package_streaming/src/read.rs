use super::ExtractError;
use std::{io::Read, path::Path};
use zip::read::read_zipfile_from_stream;

/// Returns the `.tar.bz2` as a decompressed `tar::Archive`. The `tar::Archive` can be used to
/// extract the files from it, or perform introspection.
pub fn stream_tar_bz2(reader: impl Read) -> tar::Archive<impl Read + Sized> {
    tar::Archive::new(bzip2_rs::DecoderReader::new(reader))
}

/// Returns the `.tar.zst` as a decompressed `tar` archive. The `tar::Archive` can be used to
/// extract the files from it, or perform introspection.
pub(crate) fn stream_tar_zst(
    reader: impl Read,
) -> Result<tar::Archive<impl Read + Sized>, ExtractError> {
    Ok(tar::Archive::new(zstd::stream::read::Decoder::new(reader)?))
}

/// Returns the `.tar.bz2` as a decompressed `tar` archive where the data is decoded in parallel.
///
/// `max_preread_len` defines how many bytes can be pre-read from the block. This significantly
/// speeds up the reading process, which would otherwise limit the decoder to using at most two
/// threads, independently of how many more are available. Setting a value close to zero is then
/// highly discouraged, at the same time using a value higher than the amount of available memory
/// could lead to OOM for files with a high compression ratio
#[cfg(feature = "rayon")]
pub fn parallel_stream_tar_bz2(
    reader: impl Read,
    max_preread_len: usize,
) -> tar::Archive<impl Read + Sized> {
    tar::Archive::new(bzip2_rs::ParallelDecoderReader::new(
        reader,
        bzip2_rs::RayonThreadPool,
        max_preread_len,
    ))
}

/// Extracts the contents a `.tar.bz2` package archive.
pub fn extract_tar_bz2(reader: impl Read, destination: &Path) -> Result<(), ExtractError> {
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;
    stream_tar_bz2(reader).unpack(destination)?;
    Ok(())
}

/// Extracts the contents of a `.tar.bz2` package archive while decoding the archive in parallel.
///
/// `max_preread_len` defines how many bytes can be pre-read from the block. This significantly
/// speeds up the reading process, which would otherwise limit the decoder to using at most two
/// threads, independently of how many more are available. Setting a value close to zero is then
/// highly discouraged, at the same time using a value higher than the amount of available memory
/// could lead to OOM for files with a high compression ratio
#[cfg(feature = "rayon")]
pub fn parallel_extract_tar_bz2(
    reader: impl Read,
    destination: &Path,
    max_preread_len: usize,
) -> Result<(), ExtractError> {
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;
    parallel_stream_tar_bz2(reader, max_preread_len).unpack(destination)?;
    Ok(())
}

/// Extracts the contents of a `.conda` package archive.
pub fn extract_conda(mut reader: impl Read, destination: &Path) -> Result<(), ExtractError> {
    // Construct the destination path if it doesnt exist yet
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    // Iterate over all entries in the zip-file and extract them one-by-one
    while let Some(file) = read_zipfile_from_stream(&mut reader)? {
        if file.mangled_name().ends_with(".tar.zst") {
            stream_tar_zst(file)?.unpack(destination)?;
        }
    }

    Ok(())
}
