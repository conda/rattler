//! Functions that enable extracting or streaming a Conda package for objects
//! that implement the [`tokio::io::AsyncRead`] trait.

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use futures_util::stream::StreamExt;
use tokio::io::AsyncRead;
use tokio_util::io::SyncIoBridge;

use crate::{read::SizeCountingReader, ExtractError, ExtractResult};

/// Buffer size for async I/O operations (128KB).
const DEFAULT_BUF_SIZE: usize = 128 * 1024;

/// Unix permission bits for executable files (user, group, and other execute bits).
#[cfg(unix)]
const EXECUTABLE_MODE_BITS: u32 = 0o111;

/// Unpacks a tar archive, preserving only the executable bit on Unix.
async fn unpack_tar_archive<R: tokio::io::AsyncRead + Unpin>(
    mut archive: tokio_tar::Archive<R>,
    destination: &Path,
) -> Result<(), ExtractError> {
    // Canonicalize the destination to ensure consistent path handling
    let destination = tokio::fs::canonicalize(destination)
        .await
        .map_err(ExtractError::IoError)?;

    let mut entries = archive.entries().map_err(ExtractError::IoError)?;

    // Memoize filesystem calls to canonicalize paths
    #[allow(clippy::default_trait_access)] // So we dont have to import rustc_hash
    let mut memo = Default::default();

    while let Some(entry) = entries.next().await {
        let mut file = entry.map_err(ExtractError::IoError)?;

        // On Windows, skip symlink entries as they require special privileges
        if cfg!(windows) && file.header().entry_type().is_symlink() {
            tracing::warn!(
                "Skipping symlink in tar archive: {}",
                file.path().map_err(ExtractError::IoError)?.display()
            );
            continue;
        }

        // Unpack the file into the destination directory
        #[cfg_attr(not(unix), allow(unused_variables))]
        let unpacked_path = file
            .unpack_in_raw(&destination, &mut memo)
            .await
            .map_err(ExtractError::IoError)?;

        // Preserve the executable bit on Unix systems
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let entry_type = file.header().entry_type();
            if entry_type.is_file() || entry_type.is_hard_link() {
                let mode = file.header().mode().map_err(ExtractError::IoError)?;
                let has_any_executable_bit = mode & EXECUTABLE_MODE_BITS;

                if has_any_executable_bit != 0 {
                    if let Some(path) = unpacked_path {
                        let metadata = fs_err::tokio::metadata(&path)
                            .await
                            .map_err(ExtractError::IoError)?;
                        let permissions = metadata.permissions();

                        // Only update if not already executable
                        if permissions.mode() & EXECUTABLE_MODE_BITS != EXECUTABLE_MODE_BITS {
                            fs_err::tokio::set_permissions(
                                &path,
                                std::fs::Permissions::from_mode(
                                    permissions.mode() | EXECUTABLE_MODE_BITS,
                                ),
                            )
                            .await
                            .map_err(ExtractError::IoError)?;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Extracts the contents a `.tar.bz2` package archive.
///
/// When `cas_root` is `None`, uses a fully async implementation.
/// When `cas_root` is `Some`, uses fully async CAS extraction.
pub async fn extract_tar_bz2(
    reader: impl AsyncRead + Send + Unpin + 'static,
    destination: &Path,
    cas_root: Option<&Path>,
) -> Result<ExtractResult, ExtractError> {
    use async_compression::tokio::bufread::BzDecoder;

    // Wrap the reading in additional readers that will compute the hashes while extracting
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);
    let mut size_reader = SizeCountingReader::new(&mut md5_reader);

    // Create a buffered reader for better performance
    let buf_reader = tokio::io::BufReader::with_capacity(DEFAULT_BUF_SIZE, &mut size_reader);

    // Decompress bzip2 asynchronously
    let decoder = BzDecoder::new(buf_reader);

    // Build archive with optimized settings for faster extraction:
    // - Skip mtime preservation to avoid extra syscalls
    // - Skip automatic permission handling (we'll set executable bits manually)
    // - Skip extended attributes for better performance
    let archive = tokio_tar::ArchiveBuilder::new(decoder)
        .set_preserve_mtime(true)
        .set_preserve_permissions(false)
        .set_unpack_xattrs(false)
        .build();

    // Extract the archive.
    if let Some(cas_root) = cas_root {
        rattler_cas_tar::unpack_async(archive, destination, cas_root).await?;
    } else {
        unpack_tar_archive(archive, destination).await?;
    }

    // Read the file to the end to make sure the hash is properly computed
    tokio::io::copy(&mut size_reader, &mut tokio::io::sink())
        .await
        .map_err(ExtractError::IoError)?;

    // Get the size and hashes
    let (_, total_size) = size_reader.finalize();
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    // Validate that we actually read some data from the stream.
    // If total_size is 0, it likely means the stream was truncated or the bzip2
    // decompressor silently failed without detecting an incomplete stream.
    if total_size == 0 {
        return Err(ExtractError::IoError(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "no data was read from the package stream - the stream may have been truncated",
        )));
    }

    Ok(ExtractResult {
        sha256,
        md5,
        total_size,
    })
}

/// Extracts the contents of a `.conda` package archive.
/// This will perform on-the-fly decompression by streaming the reader.
pub async fn extract_conda(
    reader: impl AsyncRead + Send + 'static,
    destination: &Path,
    cas_root: Option<&Path>,
) -> Result<ExtractResult, ExtractError> {
    extract_conda_internal(reader, destination, cas_root.map(Path::to_path_buf), false).await
}

/// Extracts the contents of a .conda package archive by fully reading the
/// stream and then decompressing
pub async fn extract_conda_via_buffering(
    reader: impl AsyncRead + Send + 'static,
    destination: &Path,
    cas_root: Option<&Path>,
) -> Result<ExtractResult, ExtractError> {
    extract_conda_internal(reader, destination, cas_root.map(Path::to_path_buf), true).await
}

/// Extracts the contents of a `.conda` package archive using the provided
/// extraction function
async fn extract_conda_internal(
    reader: impl AsyncRead + Send + 'static,
    destination: &Path,
    cas_root: Option<PathBuf>,
    use_buffering: bool,
) -> Result<ExtractResult, ExtractError> {
    // Create a async -> sync bridge
    let reader = SyncIoBridge::new(Box::pin(reader));

    // Spawn a block task to perform the extraction
    let destination = destination.to_owned();
    tokio::task::spawn_blocking(move || {
        let reader: Box<dyn Read> = Box::new(reader);
        if use_buffering {
            crate::read::extract_conda_via_buffering(reader, &destination, cas_root.as_deref())
        } else {
            crate::read::extract_conda_via_streaming(reader, &destination, cas_root.as_deref())
        }
    })
    .await
    .unwrap_or_else(|err| {
        if let Ok(reason) = err.try_into_panic() {
            std::panic::resume_unwind(reason);
        }
        Err(ExtractError::Cancelled)
    })
}
