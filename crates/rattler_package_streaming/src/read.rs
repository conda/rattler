//! Functions that enable extracting or streaming a Conda package for objects that implement the
//! [`std::io::Read`] trait.

use super::{ExtractError, ExtractResult};
use std::mem::ManuallyDrop;
use std::{ffi::OsStr, io::Read, path::Path};
use zip::read::read_zipfile_from_stream;
use std::fs::File;
use std::io::{self, Write, Seek};
use zip::read::ZipArchive;
use rattler_digest::{HashingReader, Sha256, Md5};

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
/// Extracts the contents of a `.conda` package archive.
pub fn extract_conda(reader: impl Read, destination: &Path) -> Result<ExtractResult, ExtractError> {
    // Construct the destination path if it doesn't exist yet
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    // Create a temporary file to store the downloaded zip file
    let mut temp_file = tempfile::tempfile().map_err(|e| ExtractError::IoError(e))?;
    std::io::copy(&mut reader.take(u64::MAX), &mut temp_file).map_err(|e| ExtractError::IoError(e))?;

    // Rewind the file to the beginning
    temp_file.seek(io::SeekFrom::Start(0)).map_err(|e| ExtractError::IoError(e))?;
    

    // Open the zip archive\
    let mut zip = ZipArchive::new(&mut temp_file).map_err(|e| ExtractError::ZipError(e))?;

    // Iterate over all entries in the zip-file and extract them one-by-one
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| ExtractError::ZipError(e))?;
        let outpath = destination.join(file.sanitized_name());

        if file.name().ends_with(".tar.zst") {
            let mut tar_zst_file = File::create(&outpath).map_err(|e| ExtractError::IoError(e))?;
            std::io::copy(&mut file, &mut tar_zst_file).map_err(|e| ExtractError::IoError(e))?;
            tar_zst_file.seek(io::SeekFrom::Start(0)).map_err(|e| ExtractError::IoError(e))?;
            stream_tar_zst(tar_zst_file)?.unpack(destination)?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(&p).map_err(|e| ExtractError::IoError(e))?;
                }
            }
            let mut outfile = File::create(&outpath).map_err(|e| ExtractError::IoError(e))?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| ExtractError::IoError(e))?;
        }
    }

    // Read the file to the end to make sure the hash is properly computed.
    std::io::copy(&mut temp_file, &mut std::io::sink()).map_err(|e| ExtractError::IoError(e))?;

    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new("abc");
  
    let md5_reader =
    rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);
    // Get the hashes
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    Ok(ExtractResult { sha256, md5 })
}
