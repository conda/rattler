//! Shared functionality for async extraction operations.

use std::path::Path;

use async_compression::tokio::bufread::ZstdDecoder;
use futures_util::stream::StreamExt;

use crate::ExtractError;

/// Buffer size for async I/O operations (128KB).
pub(super) const DEFAULT_BUF_SIZE: usize = 128 * 1024;

/// Unix permission bits for executable files (user, group, and other execute bits).
#[cfg(unix)]
const EXECUTABLE_MODE_BITS: u32 = 0o111;

/// Unpacks a tar archive, preserving only the executable bit on Unix.
pub(super) async fn unpack_tar_archive<R: tokio::io::AsyncRead + Unpin>(
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
                        let metadata = tokio::fs::metadata(&path)
                            .await
                            .map_err(ExtractError::IoError)?;
                        let permissions = metadata.permissions();

                        // Only update if not already executable
                        if permissions.mode() & EXECUTABLE_MODE_BITS != EXECUTABLE_MODE_BITS {
                            tokio::fs::set_permissions(
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

/// Extracts a single tar.zst entry from a ZIP file
pub(super) async fn extract_tar_zst_entry<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    destination: &Path,
) -> Result<(), ExtractError> {
    // Create a buffered reader for better performance
    let buf_reader = tokio::io::BufReader::with_capacity(DEFAULT_BUF_SIZE, &mut reader);

    // Decompress zstd asynchronously
    let decoder = ZstdDecoder::new(buf_reader);

    // Build archive with optimized settings
    let archive = tokio_tar::ArchiveBuilder::new(decoder)
        .set_preserve_mtime(true)
        .set_preserve_permissions(false)
        .set_unpack_xattrs(false)
        .set_allow_external_symlinks(false)
        .build();

    // Unpack the tar archive
    unpack_tar_archive(archive, destination).await?;

    Ok(())
}
