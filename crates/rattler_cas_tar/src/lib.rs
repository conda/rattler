//! Tar archive extraction with CAS-based file storage.
//!
//! This crate provides functions for extracting tar archives with
//! content-addressable storage (CAS) support. Regular files are written to the
//! CAS and hardlinked to the destination directory, enabling deduplication
//! across packages.
//!
//! Both synchronous ([`unpack`]) and asynchronous ([`unpack_async`]) extraction
//! are supported, controlled by the `sync` and `tokio` features respectively.
//!
//! # Features
//!
//! - `sync` (default): Enables synchronous extraction via the `tar` crate
//! - `tokio` (default): Enables async extraction via `astral-tokio-tar`

#![deny(missing_docs)]

#[cfg(any(feature = "sync", feature = "tokio"))]
use std::collections::HashSet;
use std::path::PathBuf;
#[cfg(any(feature = "sync", feature = "tokio"))]
use std::path::{Component, Path};

#[cfg(any(feature = "sync", feature = "tokio"))]
use filetime::FileTime;
#[cfg(any(feature = "sync", feature = "tokio"))]
use fs_err as fs;
#[cfg(feature = "tokio")]
use futures_util::StreamExt;

/// Errors that can occur during CAS-based tar extraction.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// An I/O error occurred.
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    /// A path traversal attempt was detected in the archive.
    #[error("path traversal attempt in archive: {0}")]
    PathTraversal(PathBuf),

    /// Failed to create a hardlink from CAS to destination.
    #[error("failed to create hardlink from CAS to {destination}")]
    HardlinkFailed {
        /// The destination path where the hardlink was supposed to be created.
        destination: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Unix permission bits for executable files (user, group, and other execute
/// bits).
#[cfg(all(unix, any(feature = "sync", feature = "tokio")))]
const EXECUTABLE_MODE_BITS: u32 = 0o111;

/// Extracts a tar archive to a destination directory, storing file contents in
/// a CAS.
///
/// Regular files are written to the CAS and hardlinked to the destination.
/// Directories, symlinks, and hardlinks are created directly in the
/// destination.
///
/// # Arguments
///
/// * `archive` - The tar archive to extract
/// * `destination` - The directory to extract files to
/// * `cas_root` - The root directory of the CAS store
///
/// # Errors
///
/// Returns an error if:
/// - Hardlinking from CAS to destination fails (e.g., cross-filesystem)
/// - Any I/O operation fails
///
/// # Security
///
/// Paths containing `..` are rejected to prevent path traversal attacks.
#[cfg(feature = "sync")]
pub fn unpack<R: std::io::Read>(
    mut archive: tar::Archive<R>,
    destination: &Path,
    cas_root: &Path,
) -> Result<(), Error> {
    // Create the destination directory if needed
    fs::create_dir_all(destination)?;

    // Canonicalize the target directory, at this point it must exist.
    let target_dir = destination
        .canonicalize()
        .expect("Destination directory exists");

    // Memoize created directories to avoid redundant syscalls
    let mut created_dirs = CreatedDirectories::new(target_dir.clone());

    for entry_result in archive.entries().map_err(Error::IoError)? {
        let mut entry = entry_result.map_err(Error::IoError)?;
        let header = entry.header().clone();
        let entry_type = header.entry_type();

        // Get and normalize the path
        let raw_path = entry.path().map_err(Error::IoError)?;
        let Some(normalized_path) = normalize_archive_path(&raw_path)? else {
            continue; // Skip "." entries
        };
        let dest_path = target_dir.join(&normalized_path);

        // Ensure the parent directory exists
        if let Some(parent) = dest_path.parent() {
            created_dirs.create_dir_all(parent)?;
        }

        // Get mtime for later
        let mtime = get_mtime(&header);

        // Handle different entry types
        if entry_type.is_dir() {
            created_dirs.create_dir_all(&dest_path)?;
            // Set mtime on directory
            if let Some(mtime) = mtime {
                let _ = filetime::set_file_mtime(&dest_path, mtime);
            }
        } else if entry_type.is_symlink() {
            // On Windows, skip symlinks as they require special privileges
            #[cfg(windows)]
            {
                tracing::warn!("Skipping symlink in tar archive: {}", raw_path.display());
            }

            #[cfg(unix)]
            {
                let link_target = header.link_name().map_err(Error::IoError)?;
                if let Some(target) = link_target {
                    // Validate that the symlink target doesn't escape the destination
                    validate_symlink_target(&normalized_path, &target)?;

                    // Try to create symlink, remove existing file only if it already exists
                    match std::os::unix::fs::symlink(&*target, &dest_path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            fs::remove_file(&dest_path).map_err(Error::IoError)?;
                            std::os::unix::fs::symlink(&*target, &dest_path)
                                .map_err(Error::IoError)?;
                        }
                        Err(e) => return Err(Error::IoError(e)),
                    }
                    // Note: symlink mtime is typically not set (and often not
                    // supported)
                }
            }
        } else if entry_type.is_hard_link() {
            // Hardlinks within the archive point to other files in the archive
            let link_target = header.link_name().map_err(Error::IoError)?;
            if let Some(target) = link_target.as_deref() {
                // Normalize the target path the same way
                let Some(normalized_target) = normalize_archive_path(target)? else {
                    continue;
                };
                let target_path = target_dir.join(&normalized_target);

                // Try to create hardlink, remove an existing file only if it already exists
                match fs::hard_link(&target_path, &dest_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        fs::remove_file(&dest_path).map_err(Error::IoError)?;
                        fs::hard_link(&target_path, &dest_path).map_err(Error::IoError)?;
                    }
                    Err(e) => return Err(Error::IoError(e)),
                }
            }
        } else if entry_type.is_file() {
            // Regular file: write to CAS and hardlink to destination

            // Get the mode before we consume the entry
            #[cfg(unix)]
            let mode = header.mode().map_err(Error::IoError)?;

            // Write to CAS
            let integrity =
                rattler_cas::write_sync(cas_root, &mut entry).map_err(Error::IoError)?;
            let cas_path = cas_root.join(rattler_cas::path_for_hash(&integrity));

            // Hardlink from CAS to destination, if a file already exists at the
            // destination, remove it.
            match fs::hard_link(&cas_path, &dest_path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    fs::remove_file(&dest_path).map_err(Error::IoError)?;
                    fs::hard_link(&cas_path, &dest_path).map_err(|e| Error::HardlinkFailed {
                        destination: dest_path.clone(),
                        source: e,
                    })?;
                }
                Err(e) => {
                    return Err(Error::HardlinkFailed {
                        destination: dest_path.clone(),
                        source: e,
                    })
                }
            }

            // Set executable bit on Unix if needed
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                if mode & EXECUTABLE_MODE_BITS != 0 {
                    let metadata = fs::metadata(&dest_path).map_err(Error::IoError)?;
                    let permissions = metadata.permissions();

                    // Only update if not already executable
                    if permissions.mode() & EXECUTABLE_MODE_BITS != EXECUTABLE_MODE_BITS {
                        fs::set_permissions(
                            &dest_path,
                            std::fs::Permissions::from_mode(
                                permissions.mode() | EXECUTABLE_MODE_BITS,
                            ),
                        )
                        .map_err(Error::IoError)?;
                    }
                }
            }

            // Set mtime
            if let Some(mtime) = mtime {
                let _ = filetime::set_file_mtime(&dest_path, mtime);
            }
        }
        // Other entry types (e.g., device files) are silently skipped
    }

    Ok(())
}

/// Asynchronously extracts a tar archive to a destination directory, storing
/// file contents in a CAS.
///
/// This is the async version of [`unpack`]. Regular files are written to the
/// CAS and hardlinked to the destination. Directories, symlinks, and hardlinks
/// are created directly in the destination.
///
/// # Arguments
///
/// * `archive` - The async tar archive to extract
/// * `destination` - The directory to extract files to
/// * `cas_root` - The root directory of the CAS store
///
/// # Errors
///
/// Returns an error if:
/// - Hardlinking from CAS to destination fails (e.g., cross-filesystem)
/// - Any I/O operation fails
///
/// # Security
///
/// Paths containing `..` are rejected to prevent path traversal attacks.
#[cfg(feature = "tokio")]
pub async fn unpack_async<R: tokio::io::AsyncRead + Unpin>(
    mut archive: tokio_tar::Archive<R>,
    destination: &Path,
    cas_root: &Path,
) -> Result<(), Error> {
    // Create the destination directory if needed
    fs_err::tokio::create_dir_all(destination).await?;

    // Canonicalize the target directory, at this point it must exist.
    let target_dir = tokio::fs::canonicalize(destination).await?;

    // Memoize created directories to avoid redundant syscalls
    let mut created_dirs = AsyncCreatedDirectories::new(target_dir.clone());

    let mut entries = archive.entries()?;
    while let Some(entry_result) = entries.next().await {
        let mut entry = entry_result?;
        let header = entry.header().clone();
        let entry_type = header.entry_type();

        // Get and normalize the path
        let raw_path = entry.path()?;
        let Some(normalized_path) = normalize_archive_path(&raw_path)? else {
            continue; // Skip "." entries
        };
        let dest_path = target_dir.join(&normalized_path);

        // Ensure the parent directory exists
        if let Some(parent) = dest_path.parent() {
            created_dirs.create_dir_all(parent).await?;
        }

        // Get mtime for later
        let mtime = get_mtime_tokio(&header);

        // Handle different entry types
        if entry_type.is_dir() {
            created_dirs.create_dir_all(&dest_path).await?;
            // Set mtime on directory
            if let Some(mtime) = mtime {
                let _ = filetime::set_file_mtime(&dest_path, mtime);
            }
        } else if entry_type.is_symlink() {
            // On Windows, skip symlinks as they require special privileges
            #[cfg(windows)]
            {
                tracing::warn!("Skipping symlink in tar archive: {}", raw_path.display());
            }

            #[cfg(unix)]
            {
                let link_target = header.link_name()?;
                if let Some(target) = link_target {
                    // Validate that the symlink target doesn't escape the destination
                    validate_symlink_target(&normalized_path, &target)?;

                    // Try to create symlink, remove existing file only if it already exists
                    match std::os::unix::fs::symlink(&*target, &dest_path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            fs_err::tokio::remove_file(&dest_path).await?;
                            std::os::unix::fs::symlink(&*target, &dest_path)?;
                        }
                        Err(e) => return Err(Error::IoError(e)),
                    }
                    // Note: symlink mtime is typically not set (and often not
                    // supported)
                }
            }
        } else if entry_type.is_hard_link() {
            // Hardlinks within the archive point to other files in the archive
            let link_target = header.link_name()?;
            if let Some(target) = link_target.as_deref() {
                // Normalize the target path the same way
                let Some(normalized_target) = normalize_archive_path(target)? else {
                    continue;
                };
                let target_path = target_dir.join(&normalized_target);

                // Try to create hardlink, remove an existing file only if it already exists
                match fs::hard_link(&target_path, &dest_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        fs_err::tokio::remove_file(&dest_path).await?;
                        fs::hard_link(&target_path, &dest_path)?;
                    }
                    Err(e) => return Err(Error::IoError(e)),
                }
            }
        } else if entry_type.is_file() {
            // Regular file: write to CAS and hardlink to destination

            // Get the mode before we consume the entry
            #[cfg(unix)]
            let mode = header.mode()?;

            // Write to CAS using async writer
            let integrity = rattler_cas::write(cas_root, &mut entry).await?;
            let cas_path = cas_root.join(rattler_cas::path_for_hash(&integrity));

            // Hardlink from CAS to destination, if a file already exists at the
            // destination, remove it.
            match fs::hard_link(&cas_path, &dest_path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    fs_err::tokio::remove_file(&dest_path).await?;
                    fs::hard_link(&cas_path, &dest_path).map_err(|e| Error::HardlinkFailed {
                        destination: dest_path.clone(),
                        source: e,
                    })?;
                }
                Err(e) => {
                    return Err(Error::HardlinkFailed {
                        destination: dest_path.clone(),
                        source: e,
                    })
                }
            }

            // Set executable bit on Unix if needed
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                if mode & EXECUTABLE_MODE_BITS != 0 {
                    let metadata = fs_err::tokio::metadata(&dest_path).await?;
                    let permissions = metadata.permissions();

                    // Only update if not already executable
                    if permissions.mode() & EXECUTABLE_MODE_BITS != EXECUTABLE_MODE_BITS {
                        fs_err::tokio::set_permissions(
                            &dest_path,
                            std::fs::Permissions::from_mode(
                                permissions.mode() | EXECUTABLE_MODE_BITS,
                            ),
                        )
                        .await?;
                    }
                }
            }

            // Set mtime
            if let Some(mtime) = mtime {
                let _ = filetime::set_file_mtime(&dest_path, mtime);
            }
        }
        // Other entry types (e.g., device files) are silently skipped
    }

    Ok(())
}

/// Normalizes a path from a tar archive by stripping leading components
/// that are not meaningful for extraction (Prefix, `RootDir`, `CurDir`).
///
/// Returns:
/// - `Ok(Some(path))` - normalized path to extract
/// - `Ok(None)` - path normalizes to nothing (e.g., "." or "/"), should be
///   skipped
/// - `Err(PathTraversal)` - path contains ".." traversal
#[cfg(any(feature = "sync", feature = "tokio"))]
fn normalize_archive_path(path: &Path) -> Result<Option<PathBuf>, Error> {
    let mut result = PathBuf::with_capacity(path.as_os_str().len());

    // Notes regarding bsdtar 2.8.3 / libarchive 2.8.3:
    // * Leading '/'s are trimmed. For example, `///test` is treated as `test`.
    // * If the filename contains '..', then the file is skipped when extracting the
    //   tarball.
    // * '//' within a filename is effectively skipped. An error is logged, but
    //   otherwise the effect is as if any two or more adjacent '/'s within the
    //   filename were consolidated into one '/'.
    //
    // Most of this is handled by the `path` module of the standard
    // library, but we specially handle a few cases here as well.
    for component in path.components() {
        match component {
            // Skip these components
            Component::Prefix(..) | Component::RootDir | Component::CurDir => {}
            // If any part of the filename is '..', then skip over unpacking the file to prevent
            // directory traversal security issues.  See, e.g.: CVE-2001-1267, CVE-2002-0399,
            // CVE-2005-1918, CVE-2007-4131
            Component::ParentDir => return Err(Error::PathTraversal(path.to_path_buf())),
            // Keep normal components
            Component::Normal(part) => result.push(part),
        }
    }

    // If the path normalized to nothing, skip it. E.g. skip empty paths.
    if result.as_os_str().is_empty() {
        return Ok(None);
    }

    Ok(Some(result))
}

/// Validates that a symlink target doesn't escape the destination directory.
///
/// # Arguments
///
/// * `normalized_source` - The normalized path of the symlink file, relative to
///   destination
/// * `target` - The symlink target from the archive
///
/// # Returns
///
/// * `Ok(())` if the symlink target is safe
/// * `Err(PathTraversal)` if the target would escape the destination
#[cfg(all(any(unix, test), any(feature = "sync", feature = "tokio")))]
#[allow(dead_code)] // Used in sync/tokio feature code paths
fn validate_symlink_target(normalized_source: &Path, target: &Path) -> Result<(), Error> {
    // Use a stack to track the resolved path depth.
    // We don't need to store actual names, just track how deep we are.
    // Start by pushing the symlink's directory components onto the stack.
    let mut result = normalized_source
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf();
    for component in target.components() {
        match component {
            // If the path contains absolute anchors, we reject it.
            Component::Prefix(..) | Component::RootDir => {
                return Err(Error::PathTraversal(target.to_path_buf()));
            }

            Component::Normal(component) => result.push(component),
            Component::ParentDir => {
                if let Some(parent) = result.parent() {
                    result = parent.to_path_buf();
                } else {
                    return Err(Error::PathTraversal(target.to_path_buf()));
                }
            }
            Component::CurDir => {}
        }
    }

    Ok(())
}

/// Gets the mtime from a tar header, handling zero values.
#[cfg(feature = "sync")]
fn get_mtime(header: &tar::Header) -> Option<FileTime> {
    get_mtime_from_raw(header.mtime().ok()?)
}

/// Gets the mtime from a `tokio_tar` header, handling zero values.
#[cfg(feature = "tokio")]
fn get_mtime_tokio(header: &tokio_tar::Header) -> Option<FileTime> {
    get_mtime_from_raw(header.mtime().ok()?)
}

/// Converts a raw mtime value to a `FileTime`, handling zero values.
#[cfg(any(feature = "sync", feature = "tokio"))]
fn get_mtime_from_raw(mtime: u64) -> Option<FileTime> {
    // Use 1 instead of 0 for compatibility (same as tar crate)
    let mtime = if mtime == 0 { 1 } else { mtime };
    Some(FileTime::from_unix_time(mtime as i64, 0))
}

/// A helper struct to memoize directory creation.
#[cfg(feature = "sync")]
struct CreatedDirectories {
    created: HashSet<PathBuf>,
    root: PathBuf,
}

#[cfg(feature = "sync")]
impl CreatedDirectories {
    fn new(root: PathBuf) -> Self {
        Self {
            created: HashSet::from_iter([root.clone()]),
            root,
        }
    }

    fn create_dir_all(&mut self, path: &Path) -> std::io::Result<()> {
        // Memoize directory creation to avoid redundant syscalls
        if !self.created.insert(path.to_path_buf()) {
            return Ok(());
        }

        // Create the parent directories
        if let Some(parent) = path.parent() {
            self.create_dir_all(parent)?;
        }

        // Create the directory itself
        match fs_err::create_dir(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }
}

// Suppress unused warning for `root` field
#[cfg(feature = "sync")]
impl Drop for CreatedDirectories {
    fn drop(&mut self) {
        let _ = &self.root;
    }
}

/// An async helper struct to memoize directory creation.
#[cfg(feature = "tokio")]
struct AsyncCreatedDirectories {
    created: HashSet<PathBuf>,
    #[allow(dead_code)]
    root: PathBuf,
}

#[cfg(feature = "tokio")]
impl AsyncCreatedDirectories {
    fn new(root: PathBuf) -> Self {
        Self {
            created: HashSet::from_iter([root.clone()]),
            root,
        }
    }

    async fn create_dir_all(&mut self, path: &Path) -> std::io::Result<()> {
        // Memoize directory creation to avoid redundant syscalls
        if !self.created.insert(path.to_path_buf()) {
            return Ok(());
        }

        // Create the parent directories
        if let Some(parent) = path.parent() {
            Box::pin(self.create_dir_all(parent)).await?;
        }

        // Create the directory itself
        match fs_err::tokio::create_dir(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Helper to build tar archives with arbitrary paths (including ones that
    /// `tar::Builder` rejects).
    mod raw_tar {
        /// Builds a tar archive with a single file entry using the given path
        /// and content. This bypasses `tar::Builder`'s path validation,
        /// allowing paths like "../" or "/absolute".
        pub fn create_archive(path: &[u8], content: &[u8]) -> Vec<u8> {
            let mut archive_data = Vec::new();

            // Build tar header manually (512 bytes)
            let mut header = [0u8; 512];

            // Name field (0-99)
            let path_len = path.len().min(100);
            header[..path_len].copy_from_slice(&path[..path_len]);

            // Mode (100-107) - 0644 in octal
            header[100..107].copy_from_slice(b"0000644");
            // UID (108-115)
            header[108..115].copy_from_slice(b"0000000");
            // GID (116-123)
            header[116..123].copy_from_slice(b"0000000");
            // Size (124-135) in octal
            let size_str = format!("{:011o}", content.len());
            header[124..135].copy_from_slice(size_str.as_bytes());
            // Mtime (136-147)
            header[136..147].copy_from_slice(b"00000000000");
            // Type flag (156) - '0' for regular file
            header[156] = b'0';
            // Magic (257-262)
            header[257..262].copy_from_slice(b"ustar");
            // Version (263-264)
            header[263..265].copy_from_slice(b"00");

            // Calculate checksum
            header[148..156].copy_from_slice(b"        ");
            let checksum: u32 = header.iter().map(|&b| u32::from(b)).sum();
            let checksum_str = format!("{checksum:06o}\0 ");
            header[148..156].copy_from_slice(checksum_str.as_bytes());

            archive_data.extend_from_slice(&header);

            // Add content (padded to 512-byte blocks)
            archive_data.extend_from_slice(content);
            let padding = (512 - (content.len() % 512)) % 512;
            archive_data.extend(std::iter::repeat_n(0u8, padding));

            // Add two empty blocks to end archive
            archive_data.extend_from_slice(&[0u8; 1024]);

            archive_data
        }

        /// Builds a tar archive with a single symlink entry.
        /// This bypasses tar::Builder's path validation.
        #[cfg(unix)]
        pub fn create_symlink_archive(path: &[u8], target: &[u8]) -> Vec<u8> {
            let mut archive_data = Vec::new();

            // Build tar header manually (512 bytes)
            let mut header = [0u8; 512];

            // Name field (0-99)
            let path_len = path.len().min(100);
            header[..path_len].copy_from_slice(&path[..path_len]);

            // Mode (100-107) - 0777 in octal for symlinks
            header[100..107].copy_from_slice(b"0000777");
            // UID (108-115)
            header[108..115].copy_from_slice(b"0000000");
            // GID (116-123)
            header[116..123].copy_from_slice(b"0000000");
            // Size (124-135) - 0 for symlinks
            header[124..135].copy_from_slice(b"00000000000");
            // Mtime (136-147)
            header[136..147].copy_from_slice(b"00000000000");
            // Type flag (156) - '2' for symlink
            header[156] = b'2';
            // Link name field (157-256) - symlink target
            let target_len = target.len().min(100);
            header[157..157 + target_len].copy_from_slice(&target[..target_len]);
            // Magic (257-262)
            header[257..262].copy_from_slice(b"ustar");
            // Version (263-264)
            header[263..265].copy_from_slice(b"00");

            // Calculate checksum
            header[148..156].copy_from_slice(b"        ");
            let checksum: u32 = header.iter().map(|&b| b as u32).sum();
            let checksum_str = format!("{:06o}\0 ", checksum);
            header[148..156].copy_from_slice(checksum_str.as_bytes());

            archive_data.extend_from_slice(&header);

            // Add two empty blocks to end archive (no content for symlinks)
            archive_data.extend_from_slice(&[0u8; 1024]);

            archive_data
        }
    }

    #[test]
    fn test_path_handling() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        // Create a tar archive with various path types
        let mut builder = tar::Builder::new(Vec::new());

        // 1. Normal file - should be extracted
        let mut header = tar::Header::new_gnu();
        header.set_path("normal/file.txt").unwrap();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_mtime(1700000000);
        header.set_cksum();
        builder.append(&header, b"hello" as &[u8]).unwrap();

        // 2. File with "./" prefix - should be normalized and extracted
        let mut header = tar::Header::new_gnu();
        header.set_path("./dotslash/file.txt").unwrap();
        header.set_size(8);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, b"dotslash" as &[u8]).unwrap();

        // 3. "." entry (current directory) - should be silently skipped
        let mut header = tar::Header::new_gnu();
        header.set_path(".").unwrap();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();

        // 4. Another normal file after the "." entry - should be extracted
        let mut header = tar::Header::new_gnu();
        header.set_path("after_dot/file.txt").unwrap();
        header.set_size(9);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, b"after_dot" as &[u8]).unwrap();

        let archive_data = builder.into_inner().unwrap();

        // Extract with CAS
        let archive = tar::Archive::new(Cursor::new(archive_data));
        let result = unpack(archive, &target_dir, &cas_dir);
        assert!(result.is_ok());

        // Verify normal file was extracted
        assert!(target_dir.join("normal/file.txt").exists());
        assert_eq!(
            std::fs::read_to_string(target_dir.join("normal/file.txt")).unwrap(),
            "hello"
        );

        // Verify dotslash file was extracted (./ prefix stripped)
        assert!(target_dir.join("dotslash/file.txt").exists());
        assert_eq!(
            std::fs::read_to_string(target_dir.join("dotslash/file.txt")).unwrap(),
            "dotslash"
        );

        // Verify file after "." entry was extracted
        assert!(target_dir.join("after_dot/file.txt").exists());
        assert_eq!(
            std::fs::read_to_string(target_dir.join("after_dot/file.txt")).unwrap(),
            "after_dot"
        );
    }

    #[test]
    fn test_file_overwrite() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        // Create first archive with original content
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_path("file.txt").unwrap();
        header.set_size(8);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, b"original" as &[u8]).unwrap();
        let archive1_data = builder.into_inner().unwrap();

        // Create second archive with updated content
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_path("file.txt").unwrap();
        header.set_size(7);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, b"updated" as &[u8]).unwrap();
        let archive2_data = builder.into_inner().unwrap();

        // Extract first archive
        let archive1 = tar::Archive::new(Cursor::new(archive1_data));
        unpack(archive1, &target_dir, &cas_dir).unwrap();

        // Verify original content
        assert_eq!(
            std::fs::read_to_string(target_dir.join("file.txt")).unwrap(),
            "original"
        );

        // Extract second archive (should overwrite)
        let archive2 = tar::Archive::new(Cursor::new(archive2_data));
        unpack(archive2, &target_dir, &cas_dir).unwrap();

        // Verify content was overwritten
        assert_eq!(
            std::fs::read_to_string(target_dir.join("file.txt")).unwrap(),
            "updated"
        );
    }

    #[test]
    fn test_path_traversal_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        let archive_data = raw_tar::create_archive(b"../escape/file.txt", b"escaped");

        let archive = tar::Archive::new(Cursor::new(archive_data));
        let result = unpack(archive, &target_dir, &cas_dir);

        assert!(result.is_err());
        assert_matches::assert_matches!(result, Err(Error::PathTraversal(_)));
    }

    #[test]
    fn test_absolute_path_normalization() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        let archive_data = raw_tar::create_archive(b"/etc/passwd", b"root:x:0:0");

        let archive = tar::Archive::new(Cursor::new(archive_data));
        let result = unpack(archive, &target_dir, &cas_dir);

        assert!(result.is_ok());

        // Verify file was extracted to normalized path (not absolute!)
        assert!(
            !Path::new("/etc/passwd").exists()
                || std::fs::read_to_string("/etc/passwd").unwrap() != "root:x:0:0"
        );
        assert!(target_dir.join("etc/passwd").exists());
        assert_eq!(
            std::fs::read_to_string(target_dir.join("etc/passwd")).unwrap(),
            "root:x:0:0"
        );
    }

    #[test]
    fn test_deduplication() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        // Create an archive with two files containing identical content
        let mut builder = tar::Builder::new(Vec::new());

        let mut header = tar::Header::new_gnu();
        header.set_path("file1.txt").unwrap();
        header.set_size(12);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, b"same content" as &[u8]).unwrap();

        let mut header = tar::Header::new_gnu();
        header.set_path("file2.txt").unwrap();
        header.set_size(12);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, b"same content" as &[u8]).unwrap();

        let archive_data = builder.into_inner().unwrap();

        // Extract with CAS
        let archive = tar::Archive::new(Cursor::new(archive_data));
        unpack(archive, &target_dir, &cas_dir).unwrap();

        // Both files should exist with same content
        assert_eq!(
            std::fs::read_to_string(target_dir.join("file1.txt")).unwrap(),
            "same content"
        );
        assert_eq!(
            std::fs::read_to_string(target_dir.join("file2.txt")).unwrap(),
            "same content"
        );

        // CAS should only have one file for the content
        let cas_files: Vec<_> = walkdir::WalkDir::new(&cas_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .collect();
        assert_eq!(cas_files.len(), 1);
    }

    /// Test that symlinks with targets escaping the destination are rejected.
    #[cfg(unix)]
    #[test]
    fn test_symlink_escape_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        // Create a symlink that tries to escape: link at "foo/link" pointing to
        // "../../etc/passwd" This would resolve to "../etc/passwd"
        // which escapes the destination
        let archive_data = raw_tar::create_symlink_archive(b"foo/link", b"../../etc/passwd");

        let archive = tar::Archive::new(Cursor::new(archive_data));
        let result = unpack(archive, &target_dir, &cas_dir);

        assert!(result.is_err());
        assert_matches::assert_matches!(result, Err(Error::PathTraversal(_)));
    }

    /// Test that symlinks with absolute targets are rejected.
    #[cfg(unix)]
    #[test]
    fn test_symlink_absolute_target_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        // Create a symlink with an absolute target
        let archive_data = raw_tar::create_symlink_archive(b"link", b"/etc/passwd");

        let archive = tar::Archive::new(Cursor::new(archive_data));
        let result = unpack(archive, &target_dir, &cas_dir);

        assert!(result.is_err());
        assert_matches::assert_matches!(result, Err(Error::PathTraversal(_)));
    }

    /// Test that valid symlinks (within destination) work correctly.
    #[cfg(unix)]
    #[test]
    fn test_symlink_valid() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        // Create a file and a symlink pointing to it
        // First create the file
        let file_data = raw_tar::create_archive(b"foo/bar/target.txt", b"target content");
        let archive = tar::Archive::new(Cursor::new(file_data));
        unpack(archive, &target_dir, &cas_dir).unwrap();

        // Now create a symlink: "foo/link" -> "bar/target.txt" (valid, stays
        // within destination)
        let symlink_data = raw_tar::create_symlink_archive(b"foo/link", b"bar/target.txt");
        let archive = tar::Archive::new(Cursor::new(symlink_data));
        let result = unpack(archive, &target_dir, &cas_dir);

        assert!(result.is_ok());
        assert!(target_dir.join("foo/link").is_symlink());
        assert_eq!(
            std::fs::read_link(target_dir.join("foo/link")).unwrap(),
            Path::new("bar/target.txt")
        );
    }

    /// Test that symlinks can use ".." as long as they stay within destination.
    #[cfg(unix)]
    #[test]
    fn test_symlink_with_parent_dir_within_bounds() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        // Create a file at the root
        let file_data = raw_tar::create_archive(b"target.txt", b"root target");
        let archive = tar::Archive::new(Cursor::new(file_data));
        unpack(archive, &target_dir, &cas_dir).unwrap();

        // Create a symlink: "foo/bar/link" -> "../../target.txt"
        // This resolves to "target.txt" which is valid (within destination)
        let symlink_data = raw_tar::create_symlink_archive(b"foo/bar/link", b"../../target.txt");
        let archive = tar::Archive::new(Cursor::new(symlink_data));
        let result = unpack(archive, &target_dir, &cas_dir);

        assert!(result.is_ok());
        assert!(target_dir.join("foo/bar/link").is_symlink());
    }

    #[test]
    fn test_directory_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_dir = temp_dir.path().join("cas");
        let target_dir = temp_dir.path().join("dest");

        // Create an archive with nested directories
        let mut builder = tar::Builder::new(Vec::new());

        let mut header = tar::Header::new_gnu();
        header.set_path("a/b/c/d/file.txt").unwrap();
        header.set_size(4);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, b"deep" as &[u8]).unwrap();

        let archive_data = builder.into_inner().unwrap();

        // Extract with CAS
        let archive = tar::Archive::new(Cursor::new(archive_data));
        unpack(archive, &target_dir, &cas_dir).unwrap();

        // Verify the nested file exists
        assert!(target_dir.join("a/b/c/d/file.txt").exists());
        assert_eq!(
            std::fs::read_to_string(target_dir.join("a/b/c/d/file.txt")).unwrap(),
            "deep"
        );
    }
}
