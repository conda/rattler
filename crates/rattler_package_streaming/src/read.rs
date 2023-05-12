//! Functions that enable extracting or streaming a Conda package for objects that implement the
//! [`std::io::Read`] trait.

use super::{ExtractError, ExtractResult};
use std::{ffi::OsStr, io::Read, path::Path};
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
pub fn extract_conda(reader: impl Read, destination: &Path) -> Result<ExtractResult, ExtractError> {
    // Construct the destination path if it doesnt exist yet
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    // Wrap the reading in aditional readers that will compute the hashes of the file while its
    // being read.
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);

    // Iterate over all entries in the zip-file and extract them one-by-one
    while let Some(file) = read_zipfile_from_stream(&mut md5_reader)? {
        if file
            .mangled_name()
            .file_name()
            .map(OsStr::to_string_lossy)
            .map_or(false, |file_name| file_name.ends_with(".tar.zst"))
        {
            stream_tar_zst(file)?.unpack(destination)?;
        }
    }

    // Read the file to the end to make sure the hash is properly computed.
    let mut buf = [0; 1 << 14];
    loop {
        let bytes_read = md5_reader.read(&mut buf)?;
        if bytes_read == 0 {
            break;
        }
    }

    // Get the hashes
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    Ok(ExtractResult { sha256, md5 })
}
