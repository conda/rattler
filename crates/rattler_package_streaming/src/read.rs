//! Functions that enable extracting or streaming a Conda package for objects that implement the
//! [`std::io::Read`] trait.

use super::{ExtractError, ExtractResult};
use std::io::{copy, Seek, SeekFrom};
use std::mem::ManuallyDrop;
use std::{ffi::OsStr, io::Read, path::Path};
use tempfile::SpooledTempFile;
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

    process_with_hashing(reader, |reader| {
        stream_tar_bz2(reader).unpack(destination)?;
        Ok(())
    })
}

/// Extracts the contents of a `.conda` package archive.
pub fn extract_conda_via_streaming(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Construct the destination path if it doesn't exist yet
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    process_with_hashing(reader, |reader| {
        while let Some(file) = read_zipfile_from_stream(reader)? {
            extract_zipfile(file, destination)?;
        }
        Ok(())
    })
}

/// Extracts the contents of a .conda package archive by fully reading the stream and then decompressing
pub fn extract_conda_via_buffering(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // delete destination first, as this method is usually used as a fallback from a failed streaming decompression
    if destination.exists() {
        std::fs::remove_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;
    }
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    process_with_hashing(reader, |reader| {
        // Create a SpooledTempFile with a 5MB limit
        let mut temp_file = SpooledTempFile::new(5 * 1024 * 1024);
        copy(reader, &mut temp_file)?;
        temp_file.seek(SeekFrom::Start(0))?;
        let mut archive = ZipArchive::new(temp_file)?;

        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            extract_zipfile(file, destination)?;
        }
        Ok(())
    })
}

fn extract_zipfile<R: std::io::Read>(
    zip_file: ZipFile<'_, R>,
    destination: &Path,
) -> Result<(), ExtractError> {
    // If an error occurs while we are reading the contents of the zip we don't want to
    // seek to the end of the file. Using [`ManuallyDrop`] we prevent `drop` to be called on
    // the `file` in case the stack unwinds.
    let mut file = ManuallyDrop::new(zip_file);

    if file
        .mangled_name()
        .file_name()
        .map(OsStr::to_string_lossy)
        .is_some_and(|file_name| file_name.ends_with(".tar.zst"))
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

// Define a custom reader to track file size
pub(crate) struct SizeCountingReader<R> {
    inner: R,
    size: u64,
}

impl<R> SizeCountingReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self { inner, size: 0 }
    }

    pub(crate) fn finalize(self) -> (R, u64) {
        (self.inner, self.size)
    }
}

impl<R: Read> Read for SizeCountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        self.size += bytes_read as u64;
        Ok(bytes_read)
    }
}

// AsyncRead implementation for use with tokio
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for SizeCountingReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let previously_filled = buf.filled().len();

        // Since R: Unpin, we can safely use get_mut
        let this = self.as_mut().get_mut();
        let reader = std::pin::Pin::new(&mut this.inner);

        match reader.poll_read(cx, buf) {
            std::task::Poll::Ready(Ok(())) => {
                let bytes_read = buf.filled().len() - previously_filled;
                this.size += bytes_read as u64;
                std::task::Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// Helper function to compute hashes and size while processing a tar archive
fn process_with_hashing<E, R, F>(reader: R, processor: F) -> Result<ExtractResult, E>
where
    R: Read,
    E: From<std::io::Error>,
    F: FnOnce(
        &mut SizeCountingReader<
            &mut rattler_digest::HashingReader<
                rattler_digest::HashingReader<R, rattler_digest::Sha256>,
                rattler_digest::Md5,
            >,
        >,
    ) -> Result<(), E>,
{
    // Wrap the reading in additional readers that will compute the hashes of the file while its
    // being read, and count the total size.
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);
    let mut size_reader = SizeCountingReader::new(&mut md5_reader);

    processor(&mut size_reader)?;

    // Read the file to the end to make sure the hash is properly computed
    std::io::copy(&mut size_reader, &mut std::io::sink())?;

    // Get the size and hashes
    let (_, total_size) = size_reader.finalize();
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    Ok(ExtractResult {
        sha256,
        md5,
        total_size,
    })
}
