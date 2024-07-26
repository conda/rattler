//! Functions that enable extracting or streaming a Conda package for objects that implement the
//! [`std::io::Read`] trait.

use super::{ExtractError, ExtractResult};
use rattler_digest::HashingReader;
use std::io::Cursor;
use std::mem::ManuallyDrop;
use std::{ffi::OsStr, io::Read, path::Path};
use zip::read::{read_zipfile_from_stream, ZipArchive, ZipFile};

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
pub fn extract_tar_bz2(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    // Wrap the reading in aditional readers that will compute the hashes of the file while its
    // being read.
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);

    // Unpack the archive
    stream_tar_bz2(&mut md5_reader).unpack(destination)?;

    // Get the hashes
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    Ok(ExtractResult { sha256, md5 })
}

/// Extracts the contents of a `.conda` package archive.
pub fn extract_conda_via_streaming(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Construct the destination path if it doesnt exist yet
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    // Wrap the reading in aditional readers that will compute the hashes of the file while its
    // being read.
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);

    // Iterate over all entries in the zip-file and extract them one-by-one
    while let Some(file) = read_zipfile_from_stream(&mut md5_reader)? {
        // If an error occurs while we are reading the contents of the zip we don't want to
        // seek to the end of the file. Using [`ManuallyDrop`] we prevent `drop` to be called on
        // the `file` in case the stack unwinds.
        let mut file = ManuallyDrop::new(file);

        if file
            .mangled_name()
            .file_name()
            .map(OsStr::to_string_lossy)
            .map_or(false, |file_name| file_name.ends_with(".tar.zst"))
        {
            stream_tar_zst(&mut *file)?.unpack(destination)?;
        } else {
            // Manually read to the end of the stream if that didn't happen.
            std::io::copy(&mut *file, &mut std::io::sink())?;
        }

        // Take the file out of the [`ManuallyDrop`] to properly drop it.
        let _ = ManuallyDrop::into_inner(file);
    }
    compute_hashes(md5_reader)
}

/// Extracts the contents of a `.conda` package archive by first fully dumping the reader
/// into a in-memory buffer, instead of streaming the reader
pub fn extract_conda_via_buffering(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // delete destination first, as this method is usually used as a fallback from a failed streaming decompression
    if destination.exists() {
        std::fs::remove_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;
    }
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    let mut buffer = Vec::new();
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);
    md5_reader.read_to_end(&mut buffer)?;

    // The stream must be seekable when decompressing zip archives using data descriptors
    let cursor = Cursor::new(buffer);

    let mut archive = ZipArchive::new(cursor)?;

    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        extract_zipfile(file, destination)?;
    }
    // Read the file to the end to make sure the hash is properly computed.
    std::io::copy(&mut md5_reader, &mut std::io::sink())?;

    compute_hashes(md5_reader)
}

fn extract_zipfile(zip_file: ZipFile<'_>, destination: &Path) -> Result<(), ExtractError> {
    // If an error occurs while we are reading the contents of the zip we don't want to
    // seek to the end of the file. Using [`ManuallyDrop`] we prevent `drop` to be called on
    // the `file` in case the stack unwinds.
    let mut file = ManuallyDrop::new(zip_file);

    if file
        .mangled_name()
        .file_name()
        .map(OsStr::to_string_lossy)
        .map_or(false, |file_name| file_name.ends_with(".tar.zst"))
    {
        stream_tar_zst(&mut *file)?.unpack(destination)?;
    } else {
        // Manually read to the end of the stream if that didn't happen.
        std::io::copy(&mut *file, &mut std::io::sink())?;
    }

    // Take the file out of the [`ManuallyDrop`] to properly drop it.
    let _ = ManuallyDrop::into_inner(file);

    Ok(())
}

fn compute_hashes<R: Read>(
    mut md5_reader: HashingReader<HashingReader<R, rattler_digest::Sha256>, rattler_digest::Md5>,
) -> Result<ExtractResult, ExtractError> {
    // Read the file to the end to make sure the hash is properly computed.
    std::io::copy(&mut md5_reader, &mut std::io::sink())?;

    // Get the hashes
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    Ok(ExtractResult { sha256, md5 })
}
