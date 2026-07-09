//! This module contains the logic to link a give file from the package cache into the target directory.
//! See [`link_file`] for more information.
use fs_err as fs;
use memmap2::Mmap;
use once_cell::sync::Lazy;
use rattler_conda_types::Platform;
use rattler_conda_types::package::{
    FileMode, OffsetGroup, OffsetRanges, PathType, PathsEntry, PrefixPlaceholder,
    select_utf8_offset_ranges,
};
use rattler_digest::Sha256;
use rattler_digest::{HashingWriter, Sha256Hash};
use reflink_copy::reflink;
use regex::Regex;
use std::borrow::Cow;
use std::fmt;
use std::fmt::Formatter;
use std::fs::Permissions;
use std::io::{BufWriter, ErrorKind, Read, Seek, Write};
use std::path::{Path, PathBuf};

use super::apple_codesign::{AppleCodeSignBehavior, codesign};
use super::{ExternalSymlinkPolicy, Prefix};

/// Describes the method to "link" a file from the source directory (or the cache directory) to the
/// destination directory.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum LinkMethod {
    /// A ref link is created from the cache to the destination. This ensures that the file does
    /// not take up more disk-space and that the file is not accidentally modified in the cache.
    Reflink,

    /// A hard link is created from the cache to the destination. This ensures that the file does
    /// not take up more disk-space but has the downside that if the file is accidentally modified
    /// it is also modified in the cache.
    Hardlink,

    /// A soft link is created. The link does not refer to the original file in the cache directory
    /// but instead it points to another file in the destination.
    Softlink,

    /// A copy of a file is created from a file in the cache directory to a file in the destination
    /// directory.
    Copy,

    /// A copy of a file is created and it is also patched.
    Patched(FileMode),
}

impl fmt::Display for LinkMethod {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            LinkMethod::Hardlink => write!(f, "hardlink"),
            LinkMethod::Softlink => write!(f, "softlink"),
            LinkMethod::Reflink => write!(f, "reflink"),
            LinkMethod::Copy => write!(f, "copy"),
            LinkMethod::Patched(FileMode::Binary) => write!(f, "binary patched"),
            LinkMethod::Patched(FileMode::Text) => write!(f, "text patched"),
        }
    }
}

/// Errors that can occur when calling [`link_file`].
#[derive(Debug, thiserror::Error)]
pub enum LinkFileError {
    /// An IO error occurred.
    #[error("unexpected io operation while {0}")]
    IoError(String, #[source] std::io::Error),

    /// The source file could not be opened.
    #[error("could not open source file for reading")]
    FailedToOpenSourceFile(#[source] std::io::Error),

    /// The source file could not be opened.
    #[error("failed to read the source file")]
    FailedToReadSourceFile(#[source] std::io::Error),

    /// Unable to read the contents of a symlink
    #[error("could not open source file")]
    FailedToReadSymlink(#[source] std::io::Error),

    /// Linking the file from the source to the destination failed.
    #[error("failed to {0} file to destination")]
    FailedToLink(LinkMethod, #[source] std::io::Error),

    /// The source file metadata could not be read.
    #[error("could not source file metadata")]
    FailedToReadSourceFileMetadata(#[source] std::io::Error),

    /// The destination file could not be opened.
    #[error("could not open destination file for writing")]
    FailedToOpenDestinationFile(#[source] std::io::Error),

    /// The permissions could not be updated on the destination file.
    #[error("could not update destination file permissions")]
    FailedToUpdateDestinationFilePermissions(#[source] std::io::Error),

    /// The atime/mtime could not be updated on the destination file.
    #[error("could not update file modification and access time")]
    FailedToUpdateDestinationFileTimestamps(#[source] std::io::Error),

    /// The binary (dylib or executable) could not be signed (codesign -f -s -) on
    /// macOS ARM64 (Apple Silicon).
    #[error("failed to sign Apple binary")]
    FailedToSignAppleBinary,

    /// The symlink target escapes the target prefix directory.
    #[error("symlink target escapes the target prefix")]
    SymlinkTargetEscapesPrefix,

    /// No Python version was specified when installing a noarch package.
    #[error("cannot install noarch python files because there is no python version specified ")]
    MissingPythonInfo,

    /// The hash of the file could not be computed.
    #[error("failed to compute the sha256 hash of the file")]
    FailedToComputeSha(#[source] std::io::Error),
}

/// The successful result of calling [`link_file`].
#[derive(Debug)]
pub struct LinkedFile {
    /// True if an existing file already existed and linking overwrote the original file.
    pub clobbered: bool,

    /// The SHA256 hash of the resulting file.
    pub sha256: rattler_digest::Sha256Hash,

    /// The size of the final file in bytes.
    pub file_size: u64,

    /// The relative path of the file in the destination directory. This might be different from the
    /// relative path in the source directory for python noarch packages.
    pub relative_path: PathBuf,

    /// The way the file was linked
    pub method: LinkMethod,

    /// The original prefix placeholder that was replaced
    pub prefix_placeholder: Option<String>,
}

/// Installs a single file from a `package_dir` to the the `target_dir`. Replaces any
/// `prefix_placeholder` in the file with the `prefix`.
///
/// `relative_path` is the path of the file in the `package_dir` (and the `target_dir`).
///
/// Note that usually the `target_prefix` is equal to `target_dir` but it might differ. See
/// [`crate::install::InstallOptions::target_prefix`] for more information.
///
/// The `modification_time` is a timestamp we set on all files we modify. We want a value
/// we control here to make the generated filesystem tree more reproducible. `modification_time`
/// should be greater than any of the modification times of any of the files that were packaged
/// up (ignoring any data conda stores).
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub fn link_file(
    path_json_entry: &PathsEntry,
    destination_relative_path: PathBuf,
    package_dir: &Path,
    target_dir: &Prefix,
    target_prefix: &str,
    allow_symbolic_links: bool,
    allow_hard_links: bool,
    allow_ref_links: bool,
    target_platform: Platform,
    apple_codesign_behavior: AppleCodeSignBehavior,
    modification_time: filetime::FileTime,
    external_symlink_policy: ExternalSymlinkPolicy,
) -> Result<Option<LinkedFile>, LinkFileError> {
    let source_path = package_dir.join(&path_json_entry.relative_path);

    let destination_path = target_dir.path().join(&destination_relative_path);

    // Temporary variables to store intermediate computations in. If we already computed the file
    // size or the sha hash we don't have to recompute them at the end of the function.
    let mut sha256 = None;
    let mut file_size = path_json_entry.size_in_bytes;

    let link_method = if let Some(PrefixPlaceholder {
        file_mode,
        placeholder,
        experimental_offsets: offsets,
        experimental_shebang_length: shebang_length,
    }) = path_json_entry.prefix_placeholder.as_ref()
    {
        // Memory map the source file. This provides us with easy access to a continuous stream of
        // bytes which makes it easier to search for the placeholder prefix.
        let source = map_or_read_source_file(&source_path)?;

        // Detect file type from the content
        let file_type = FileType::detect(source.as_ref());

        // Open the destination file
        let destination = BufWriter::with_capacity(
            50 * 1024,
            fs::File::create(&destination_path)
                .map_err(LinkFileError::FailedToOpenDestinationFile)?,
        );
        let mut destination_writer = HashingWriter::<_, rattler_digest::Sha256>::new(destination);

        // Convert back-slashes (\) on windows with forward-slashes (/) to avoid problems with
        // string escaping. For instance if we replace the prefix in the following text
        //
        // ```text
        // string = "c:\\old_prefix"
        // ```
        //
        // with the path `c:\new_prefix` the text will become:
        //
        // ```text
        // string = "c:\new_prefix"
        // ```
        //
        // In this case the literal string is not properly escape. This is fixed by using
        // forward-slashes on windows instead.
        let target_prefix = if target_platform.is_windows() {
            Cow::Owned(target_prefix.replace('\\', "/"))
        } else {
            Cow::Borrowed(target_prefix)
        };

        // depending on the availability of the offsets
        match offsets {
            Some(offsets) => {
                // The offsets/`shebang_length` come from the (untrusted) `paths.json`. If they are
                // inconsistent with the file contents we do not fail the install: we fall back to
                // the search-based path with a warning. The offset function guarantees it wrote
                // nothing before reporting an inconsistency, so the fallback reuses the same
                // (empty) destination.
                match copy_and_replace_placeholders_with_offsets(
                    source.as_ref(),
                    &mut destination_writer,
                    placeholder,
                    &target_prefix,
                    &target_platform,
                    *file_mode,
                    offsets,
                    *shebang_length,
                ) {
                    Ok(()) => {}
                    Err(OffsetReplaceError::InconsistentMetadata(reason)) => {
                        tracing::warn!(
                            "prefix replacement offsets for '{}' are inconsistent ({reason}); \
                             falling back to search-based replacement",
                            path_json_entry.relative_path.display()
                        );
                        copy_and_replace_placeholders(
                            source.as_ref(),
                            &mut destination_writer,
                            placeholder,
                            &target_prefix,
                            &target_platform,
                            *file_mode,
                        )
                        .map_err(|err| {
                            LinkFileError::IoError(String::from("replacing placeholders"), err)
                        })?;
                    }
                    Err(OffsetReplaceError::Io(err)) => {
                        return Err(LinkFileError::IoError(
                            String::from("replacing placeholders"),
                            err,
                        ));
                    }
                }
            }
            None => {
                // Replace the prefix placeholder in the file with the new placeholder
                copy_and_replace_placeholders(
                    source.as_ref(),
                    &mut destination_writer,
                    placeholder,
                    &target_prefix,
                    &target_platform,
                    *file_mode,
                )
                .map_err(|err| {
                    LinkFileError::IoError(String::from("replacing placeholders"), err)
                })?;
            }
        }

        let (mut file, current_hash) = destination_writer.finalize();

        // We computed the hash of the file while writing and from the file we can also infer the
        // size of it.
        sha256 = Some(current_hash);
        file_size = file.stream_position().ok();

        // We no longer need the file.
        drop(file);

        let metadata = fs::symlink_metadata(&source_path)
            .map_err(LinkFileError::FailedToReadSourceFileMetadata)?;

        let executable = has_executable_permissions(&metadata.permissions());

        // (re)sign the binary if the file is executable or is a Mach-O binary (e.g., dylib)
        // This is required for all macOS platforms because prefix replacement modifies the binary
        // content, which invalidates existing signatures. We need to preserve entitlements.
        if (executable || file_type == Some(FileType::MachO))
            && target_platform.is_osx()
            && *file_mode == FileMode::Binary
        {
            // Did the binary actually change?
            let mut content_changed = false;
            if let Some(original_hash) = &path_json_entry.sha256 {
                content_changed = original_hash != &current_hash;
            }

            // If the binary changed it requires resigning.
            if content_changed && apple_codesign_behavior != AppleCodeSignBehavior::DoNothing {
                match codesign(&destination_path) {
                    Ok(_) => {}
                    Err(e) => {
                        if apple_codesign_behavior == AppleCodeSignBehavior::Fail {
                            return Err(e);
                        }
                    }
                }

                // The file on disk changed from the original file so the hash and file size
                // also became invalid. Let's recompute them.
                sha256 = Some(
                    rattler_digest::compute_file_digest::<Sha256>(&destination_path)
                        .map_err(LinkFileError::FailedToComputeSha)?,
                );
                file_size = Some(
                    fs::symlink_metadata(&destination_path)
                        .map_err(LinkFileError::FailedToOpenDestinationFile)?
                        .len(),
                );
            }
        }

        // Copy file permissions and timestamps
        fs::set_permissions(&destination_path, metadata.permissions())
            .map_err(LinkFileError::FailedToUpdateDestinationFilePermissions)?;
        filetime::set_file_times(&destination_path, modification_time, modification_time)
            .map_err(LinkFileError::FailedToUpdateDestinationFileTimestamps)?;

        LinkMethod::Patched(*file_mode)
    } else if path_json_entry.path_type == PathType::HardLink && allow_ref_links {
        reflink_to_destination(&source_path, &destination_path, allow_hard_links)?
    } else if path_json_entry.path_type == PathType::HardLink && allow_hard_links {
        hardlink_to_destination(&source_path, &destination_path)?
    } else if path_json_entry.path_type == PathType::SoftLink {
        // The source for a SoftLink may be missing if extraction skipped it (this
        // happens on Windows when the user lacks the privileges required to create
        // symlinks; see `rattler_package_streaming::tokio::shared`). Convert the
        // resulting NotFound into a skip rather than failing the whole install.
        let dispatch = if allow_symbolic_links {
            symlink_to_destination(
                &source_path,
                &destination_path,
                target_dir.path(),
                external_symlink_policy,
            )
        } else {
            copy_symlink_target_to_destination(&source_path, &destination_path)
        };
        match dispatch {
            Ok(method) => method,
            Err(LinkFileError::FailedToReadSymlink(io) | LinkFileError::FailedToLink(_, io))
                if io.kind() == ErrorKind::NotFound =>
            {
                tracing::warn!(
                    "skipping symlink entry '{}': source missing in package cache (likely skipped during extraction)",
                    path_json_entry.relative_path.display()
                );
                return Ok(None);
            }
            Err(e) => return Err(e),
        }
    } else {
        copy_to_destination(&source_path, &destination_path)?
    };

    // Compute the final SHA256 if we didn't already or if its not stored in the paths.json entry.
    let sha256 = if let Some(sha256) = sha256 {
        sha256
    } else if link_method == LinkMethod::Softlink {
        // we hash the content of the symlink file. Note that this behavior is different from
        // conda or mamba (where the target of the symlink is hashed). However, hashing the target
        // of the symlink is more tricky in our case as we link everything in parallel and would have to
        // potentially "wait" for dependencies to be available.
        // This needs to be taken into account when verifying an installation.
        let linked_path = destination_path
            .read_link()
            .map_err(LinkFileError::FailedToReadSymlink)?;
        rattler_digest::compute_bytes_digest::<Sha256>(
            linked_path.as_os_str().to_string_lossy().as_bytes(),
        )
    } else if let Some(sha256) = path_json_entry.sha256 {
        sha256
    } else if path_json_entry.path_type == PathType::HardLink {
        rattler_digest::compute_file_digest::<Sha256>(&destination_path)
            .map_err(LinkFileError::FailedToComputeSha)?
    } else {
        // This is either a softlink or a directory.
        // Computing the hash for a directory is not possible.
        // This hash is `0000...0000`
        Sha256Hash::default()
    };

    // Compute the final file size if we didn't already.
    let file_size = if let Some(file_size) = file_size {
        file_size
    } else if let Some(size_in_bytes) = path_json_entry.size_in_bytes {
        size_in_bytes
    } else {
        let metadata = fs::symlink_metadata(&destination_path)
            .map_err(LinkFileError::FailedToOpenDestinationFile)?;
        metadata.len()
    };

    let prefix_placeholder: Option<String> = path_json_entry
        .prefix_placeholder
        .as_ref()
        .map(|p| p.placeholder.clone());

    Ok(Some(LinkedFile {
        clobbered: false,
        sha256,
        file_size,
        relative_path: destination_relative_path,
        method: link_method,
        prefix_placeholder,
    }))
}

/// Either a memory mapped file or the complete contents of a file read to memory.
enum MmapOrBytes {
    Mmap(Mmap),
    Bytes(Vec<u8>),
}

impl AsRef<[u8]> for MmapOrBytes {
    fn as_ref(&self) -> &[u8] {
        match &self {
            MmapOrBytes::Mmap(mmap) => mmap.as_ref(),
            MmapOrBytes::Bytes(bytes) => bytes.as_slice(),
        }
    }
}

/// Either memory maps, or reads the contents of the file at the specified location.
///
/// This method prefers to memory map the file to reduce the memory load but if memory mapping fails
/// it falls back to reading the contents of the file.
///
/// This fallback exists because we've seen that in some particular situations memory mapping is not
/// allowed. A particular dubious case we've encountered is described in the this issue:
/// <https://github.com/prefix-dev/pixi/issues/234>
#[allow(clippy::verbose_file_reads)]
fn map_or_read_source_file(source_path: &Path) -> Result<MmapOrBytes, LinkFileError> {
    let mut file = fs::File::open(source_path).map_err(LinkFileError::FailedToOpenSourceFile)?;

    // Try to memory map the file
    let mmap = unsafe { Mmap::map(&file) };

    // If memory mapping the file failed for whatever reason, try reading it directly to
    // memory instead.
    Ok(match mmap {
        Ok(memory) => MmapOrBytes::Mmap(memory),
        Err(err) => {
            tracing::warn!(
                "failed to memory map {}: {err}. Reading the file to memory instead.",
                source_path.display()
            );
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(LinkFileError::FailedToReadSourceFile)?;
            MmapOrBytes::Bytes(bytes)
        }
    })
}

/// Reflink (Copy-On-Write) the specified file from the source (or cached) directory. If the file
/// already exists it is removed and the operation is retried.
fn reflink_to_destination(
    source_path: &Path,
    destination_path: &Path,
    allow_hard_links: bool,
) -> Result<LinkMethod, LinkFileError> {
    loop {
        match reflink(source_path, destination_path) {
            Ok(_) => {
                #[cfg(not(target_os = "macos"))]
                {
                    // Mac is documented to clone the file attributes and extended attributes. Linux and Windows
                    // both do not guarantee that, so copy permissions and timestamps
                    let metadata = fs::symlink_metadata(source_path)
                        .map_err(LinkFileError::FailedToReadSourceFileMetadata)?;
                    fs::set_permissions(destination_path, metadata.permissions())
                        .map_err(LinkFileError::FailedToUpdateDestinationFilePermissions)?;
                    let file_time = filetime::FileTime::from_last_modification_time(&metadata);
                    filetime::set_file_times(destination_path, file_time, file_time)
                        .map_err(LinkFileError::FailedToUpdateDestinationFileTimestamps)?;
                }

                return Ok(LinkMethod::Reflink);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                fs::remove_file(destination_path).map_err(|err| {
                    LinkFileError::IoError(String::from("removing clobbered file"), err)
                })?;
            }
            Err(e) if e.kind() == ErrorKind::Unsupported && allow_hard_links => {
                return hardlink_to_destination(source_path, destination_path);
            }
            Err(e) if e.kind() == ErrorKind::Unsupported && !allow_hard_links => {
                return copy_to_destination(source_path, destination_path);
            }
            Err(_) => {
                return if allow_hard_links {
                    hardlink_to_destination(source_path, destination_path)
                } else {
                    copy_to_destination(source_path, destination_path)
                };
            }
        }
    }
}

/// Hard link the specified file from the source (or cached) directory. If the file already exists
/// it is removed and the operation is retried.
fn hardlink_to_destination(
    source_path: &Path,
    destination_path: &Path,
) -> Result<LinkMethod, LinkFileError> {
    loop {
        match fs::hard_link(source_path, destination_path) {
            Ok(_) => {
                // No need to copy file permissions, hard links share those anyway
                return Ok(LinkMethod::Hardlink);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                fs::remove_file(destination_path).map_err(|err| {
                    LinkFileError::IoError(String::from("removing clobbered file"), err)
                })?;
            }
            Err(e) => {
                tracing::debug!(
                    "failed to hardlink {}: {e}, falling back to copying.",
                    destination_path.display()
                );
                return copy_to_destination(source_path, destination_path);
            }
        }
    }
}

/// Symlink the specified file from the source (or cached) directory. If the file already exists it
/// is removed and the operation is retried.
fn symlink_to_destination(
    source_path: &Path,
    destination_path: &Path,
    target_prefix: &Path,
    external_symlink_policy: ExternalSymlinkPolicy,
) -> Result<LinkMethod, LinkFileError> {
    let linked_path = source_path
        .read_link()
        .map_err(LinkFileError::FailedToReadSymlink)?;

    // Resolve the symlink target relative to the destination's parent and
    // verify it stays inside the target prefix.
    let resolved = destination_path
        .parent()
        .unwrap_or(destination_path)
        .join(&linked_path);

    let mut normalized = PathBuf::new();
    for component in resolved.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }

    if !normalized.starts_with(target_prefix) {
        match external_symlink_policy {
            ExternalSymlinkPolicy::Allow => {}
            ExternalSymlinkPolicy::Warn => {
                tracing::warn!(
                    "symlink {} points outside the target prefix: {}",
                    destination_path.display(),
                    linked_path.display()
                );
            }
            ExternalSymlinkPolicy::Deny => {
                return Err(LinkFileError::SymlinkTargetEscapesPrefix);
            }
        }
    }

    loop {
        match symlink(&linked_path, destination_path) {
            Ok(_) => {
                // Copy timestamps as permissions are not relevant on soft links
                let metadata = fs::symlink_metadata(source_path)
                    .map_err(LinkFileError::FailedToReadSourceFileMetadata)?;
                let file_time = filetime::FileTime::from_last_modification_time(&metadata);
                filetime::set_symlink_file_times(destination_path, file_time, file_time)
                    .map_err(LinkFileError::FailedToUpdateDestinationFileTimestamps)?;

                return Ok(LinkMethod::Softlink);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                fs::remove_file(destination_path).map_err(|err| {
                    LinkFileError::IoError(String::from("removing clobbered file"), err)
                })?;
            }
            Err(e) => {
                tracing::debug!(
                    "failed to symlink {}: {e}, falling back to copying.",
                    destination_path.display()
                );
                return copy_symlink_target_to_destination(source_path, destination_path);
            }
        }
    }
}

/// Copy the specified file from the source (or cached) directory. If the file already exists it is
/// removed and the operation is retried.
fn copy_to_destination(
    source_path: &Path,
    destination_path: &Path,
) -> Result<LinkMethod, LinkFileError> {
    loop {
        match fs::copy(source_path, destination_path) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // If the file already exists, remove it and try again.
                fs::remove_file(destination_path).map_err(|err| {
                    LinkFileError::IoError(String::from("removing clobbered file"), err)
                })?;
            }
            Ok(_) => {
                // Copy file modification times, fs::copy transfers file permissions automatically
                let metadata = fs::symlink_metadata(source_path)
                    .map_err(LinkFileError::FailedToReadSourceFileMetadata)?;
                let file_time = filetime::FileTime::from_last_modification_time(&metadata);
                filetime::set_file_times(destination_path, file_time, file_time)
                    .map_err(LinkFileError::FailedToUpdateDestinationFileTimestamps)?;

                return Ok(LinkMethod::Copy);
            }
            Err(e) => return Err(LinkFileError::FailedToLink(LinkMethod::Copy, e)),
        }
    }
}

/// Copy the file a cached symlink points to. `fs::copy` on the symlink itself
/// fails when its target is only valid in the install prefix, so we resolve
/// through the cache first.
fn copy_symlink_target_to_destination(
    source_path: &Path,
    destination_path: &Path,
) -> Result<LinkMethod, LinkFileError> {
    let resolved = fs::canonicalize(source_path).map_err(LinkFileError::FailedToReadSymlink)?;
    copy_to_destination(&resolved, destination_path)
}

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text.
///
/// This switches to more specialized functions that handle the replacement of either
/// textual and binary placeholders, the [`FileMode`] enum switches between the two functions.
/// See both [`copy_and_replace_cstring_placeholder`] and [`copy_and_replace_textual_placeholder`]
pub fn copy_and_replace_placeholders(
    source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Platform,
    file_mode: FileMode,
) -> Result<(), std::io::Error> {
    match file_mode {
        FileMode::Text => {
            copy_and_replace_textual_placeholder(
                source_bytes,
                destination,
                prefix_placeholder,
                target_prefix,
                target_platform,
            )?;
        }
        FileMode::Binary => {
            // conda does not replace the prefix in the binary files on windows
            // DLLs are loaded quite differently anyways (there is no rpath, for example).
            if target_platform.is_windows() {
                destination.write_all(source_bytes)?;
            } else {
                copy_and_replace_cstring_placeholder(
                    source_bytes,
                    destination,
                    prefix_placeholder,
                    target_prefix,
                )?;
            }
        }
    }
    Ok(())
}

/// Error returned by the offset-based prefix replacement functions
/// ([`copy_and_replace_placeholders_with_offsets`] and the specialized
/// text/binary variants it dispatches to).
///
/// The `offsets` and `shebang_length` recorded in `paths.json` come from the
/// package producer and are not trusted. When they are inconsistent with the
/// file contents or with each other (see the CEP "Prefix placeholder offsets
/// in `paths.json`"), the install MUST NOT fail: the caller falls back to the
/// search-based replacement path. Genuine IO errors that occur while writing
/// the patched file are surfaced separately so they are not mistaken for
/// producer non-conformance.
///
/// The offset functions guarantee that they write nothing to the destination
/// before returning [`OffsetReplaceError::InconsistentMetadata`]; this is what
/// lets the caller reuse the same (still empty) destination for the fallback.
#[derive(Debug, thiserror::Error)]
pub enum OffsetReplaceError {
    /// The recorded `offsets`/`shebang_length` are inconsistent with the file
    /// contents or with each other. Callers SHOULD fall back to search-based
    /// replacement rather than failing the install.
    #[error("inconsistent prefix replacement metadata: {0}")]
    InconsistentMetadata(String),

    /// A genuine IO error occurred while writing the patched file.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl OffsetReplaceError {
    fn inconsistent(msg: impl Into<String>) -> Self {
        OffsetReplaceError::InconsistentMetadata(msg.into())
    }
}

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text, using the offset groups recorded in
/// `paths.json` instead of searching the file contents.
///
/// Per the CEP, an installer applies exactly the groups whose encodings its own search-based
/// replacement covers. rattler's search-based replacement covers UTF-8 only, so the UTF-8 group's
/// ranges (selected and structurally validated by [`select_utf8_offset_ranges`]) are spliced by
/// [`copy_and_replace_textual_placeholder_offsets`] or
/// [`copy_and_replace_cstring_placeholder_offsets`]; groups for the other defined encodings are
/// skipped, since their occurrences would not have been replaced by the search either. Valid
/// metadata without a UTF-8 group means there is nothing to splice: the file is copied through
/// unchanged apart from the shebang handling of text files.
///
/// `shebang_length` bounds the leading shebang region for text files and is ignored for binary
/// files. Returns [`OffsetReplaceError::InconsistentMetadata`] (having written nothing) when the
/// metadata does not match the file, so the caller can fall back to search-based replacement.
#[allow(clippy::too_many_arguments)]
pub fn copy_and_replace_placeholders_with_offsets(
    source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Platform,
    file_mode: FileMode,
    offsets: &[OffsetGroup],
    shebang_length: Option<usize>,
) -> Result<(), OffsetReplaceError> {
    let ranges = select_utf8_offset_ranges(offsets, file_mode, shebang_length.is_some())
        .map_err(|err| OffsetReplaceError::inconsistent(err.to_string()))?;

    match (file_mode, ranges) {
        (FileMode::Text, None | Some(OffsetRanges::Text(_))) => {
            // With no UTF-8 group, only the shebang region transforms (still
            // validated against `shebang_length`); the body copies verbatim.
            let body_offsets = match ranges {
                Some(OffsetRanges::Text(offsets)) => offsets.as_slice(),
                _ => &[],
            };
            copy_and_replace_textual_placeholder_offsets(
                source_bytes,
                destination,
                prefix_placeholder,
                target_prefix,
                target_platform,
                body_offsets,
                shebang_length,
            )?;
        }
        (FileMode::Binary, ranges) => {
            // conda does not replace the prefix in the binary files on windows
            // DLLs are loaded quite differently anyways (there is no rpath, for example).
            match ranges {
                Some(OffsetRanges::Binary(groups)) if !target_platform.is_windows() => {
                    copy_and_replace_cstring_placeholder_offsets(
                        source_bytes,
                        destination,
                        prefix_placeholder,
                        target_prefix,
                        groups,
                    )?;
                }
                None | Some(OffsetRanges::Binary(_)) => {
                    destination.write_all(source_bytes)?;
                }
                Some(OffsetRanges::Text(_)) => {
                    // Unreachable: the shape is validated by `select_utf8_offset_ranges`.
                    return Err(OffsetReplaceError::inconsistent(
                        "ranges shape does not match file mode",
                    ));
                }
            }
        }
        (FileMode::Text, Some(OffsetRanges::Binary(_))) => {
            // Unreachable: the shape is validated by `select_utf8_offset_ranges`.
            return Err(OffsetReplaceError::inconsistent(
                "ranges shape does not match file mode",
            ));
        }
    }
    Ok(())
}

static SHEBANG_REGEX: Lazy<Regex> = Lazy::new(|| {
    // ^(#!      pretty much the whole match string
    // (?:[ ]*)  allow spaces between #! and beginning of
    //           the executable path
    // (/(?:\\ |[^ \n\r\t])*)  the executable is the next
    //                         text block without an
    //                         escaped space or non-space
    //                         whitespace character
    // (.*))$    the rest of the line can contain option
    //           flags and end whole_shebang group
    Regex::new(r"^(#!(?:[ ]*)(/(?:\\ |[^ \n\r\t])*)(.*))$").unwrap()
});

static PYTHON_REGEX: Lazy<Regex> = Lazy::new(|| {
    // Match string starting with `python`, and optional version number
    // followed by optional flags.
    // python matches the string `python`
    // (?:\d+(?:\.\d+)*)? matches an optional version number
    Regex::new(r"^python(?:\d+(?:\.\d+)?)?$").unwrap()
});

/// Finds if the shebang line length is valid.
fn is_valid_shebang_length(shebang: &str, platform: &Platform) -> bool {
    const MAX_SHEBANG_LENGTH_LINUX: usize = 127;
    const MAX_SHEBANG_LENGTH_MACOS: usize = 512;

    if platform.is_linux() {
        shebang.len() <= MAX_SHEBANG_LENGTH_LINUX
    } else if platform.is_osx() {
        shebang.len() <= MAX_SHEBANG_LENGTH_MACOS
    } else {
        true
    }
}

/// Convert a shebang to use `/usr/bin/env` to find the executable.
/// This is useful for long shebangs or shebangs with spaces.
fn convert_shebang_to_env(shebang: Cow<'_, str>) -> Cow<'_, str> {
    if let Some(captures) = SHEBANG_REGEX.captures(&shebang) {
        let path = &captures[2];
        let exe_name = path.rsplit_once('/').map_or(path, |(_, f)| f);
        if PYTHON_REGEX.is_match(exe_name) {
            Cow::Owned(format!(
                "#!/bin/sh\n'''exec' \"{}\"{} \"$0\" \"$@\" #'''",
                path, &captures[3]
            ))
        } else {
            Cow::Owned(format!("#!/usr/bin/env {}{}", exe_name, &captures[3]))
        }
    } else {
        shebang
    }
}

/// Long shebangs and shebangs with spaces are invalid.
/// Long shebangs are longer than 127 on Linux or 512 on macOS characters.
/// Shebangs with spaces are replaced with a shebang that uses `/usr/bin/env` to find the executable.
/// This function replaces long shebangs with a shebang that uses `/usr/bin/env` to find the
/// executable.
fn replace_shebang<'a>(
    shebang: Cow<'a, str>,
    old_new: (&str, &str),
    platform: &Platform,
) -> Cow<'a, str> {
    // If the new shebang would contain a space, return a `#!/usr/bin/env` shebang
    assert!(
        shebang.starts_with("#!"),
        "Shebang does not start with #! ({shebang})",
    );

    if old_new.1.contains(' ') {
        // Doesn't matter if we don't replace anything
        if !shebang.contains(old_new.0) {
            return shebang;
        }
        // we convert the shebang without spaces to a new shebang, and only then replace
        // which is relevant for the Python case
        let new_shebang = convert_shebang_to_env(shebang).replace(old_new.0, old_new.1);
        return new_shebang.into();
    }

    let shebang: Cow<'_, str> = shebang.replace(old_new.0, old_new.1).into();

    if !shebang.starts_with("#!") {
        tracing::warn!("Shebang does not start with #! ({})", shebang);
        return shebang;
    }

    if is_valid_shebang_length(&shebang, platform) {
        shebang
    } else {
        convert_shebang_to_env(shebang)
    }
}

/// Transform the shebang region (the first `shebang_length` bytes of a text file) exactly as the
/// installer does when writing the patched file, returning the region's contribution to the output.
///
/// On targets with shebang handling ([`Platform::is_unix`]) the region minus its trailing newline
/// is rewritten by `replace_shebang` (which may collapse an over-long line to the
/// `#!/usr/bin/env <program>` form) and the trailing newline, if present, is appended unchanged. On
/// other targets the region receives plain placeholder replacement (searching at most
/// `shebang_length` bytes).
///
/// This is the single source of truth for how the shebang region is transformed, shared by the
/// install-time replacement here and the mount-time ranged reads in `rattler_vfs`, so the two stay
/// byte-identical.
pub fn replace_shebang_region(
    region: &[u8],
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Platform,
) -> Vec<u8> {
    if region.is_empty() {
        return Vec::new();
    }

    if target_platform.is_unix() {
        // Feed the region minus its trailing newline to the shebang rules; the newline byte, when
        // present, is appended unchanged.
        let has_newline = region[region.len() - 1] == b'\n';
        let line_end = if has_newline {
            region.len() - 1
        } else {
            region.len()
        };
        let first_line = String::from_utf8_lossy(&region[..line_end]);
        let new_shebang = replace_shebang(
            first_line,
            (prefix_placeholder, target_prefix),
            target_platform,
        );
        let mut out = new_shebang.into_owned().into_bytes();
        if has_newline {
            out.extend_from_slice(&region[line_end..]);
        }
        out
    } else {
        // Non-rewriting target (e.g. Windows for a noarch package): plain placeholder replacement.
        let old_prefix = prefix_placeholder.as_bytes();
        let new_prefix = target_prefix.as_bytes();
        let mut out = Vec::with_capacity(region.len());
        let mut last = 0;
        for index in memchr::memmem::find_iter(region, old_prefix) {
            out.extend_from_slice(&region[last..index]);
            out.extend_from_slice(new_prefix);
            last = index + old_prefix.len();
        }
        out.extend_from_slice(&region[last..]);
        out
    }
}

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text.
///
/// This is a text based version where the complete string is replaced. This works fine for text
/// files but will not work correctly for binary files where the length of the string is often
/// important. See [`copy_and_replace_cstring_placeholder`] when you are dealing with binary
/// content.
pub fn copy_and_replace_textual_placeholder(
    mut source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Platform,
) -> Result<(), std::io::Error> {
    // Get the prefixes as bytes
    let old_prefix = prefix_placeholder.as_bytes();
    let new_prefix = target_prefix.as_bytes();

    // check if we have a shebang. We need to handle it differently because it has a maximum length
    // that can be exceeded in very long target prefix's.
    if target_platform.is_unix() && source_bytes.starts_with(b"#!") {
        // Extract the first line. When the file has no newline the whole file is
        // the shebang line; using the file length (rather than `0`) keeps the
        // `#!` prefix in `first_line` so `replace_shebang`'s `starts_with("#!")`
        // assertion holds instead of panicking.
        let (first, rest) = source_bytes.split_at(
            source_bytes
                .iter()
                .position(|&c| c == b'\n')
                .unwrap_or(source_bytes.len()),
        );
        let first_line = String::from_utf8_lossy(first);
        let new_shebang = replace_shebang(
            first_line,
            (prefix_placeholder, target_prefix),
            target_platform,
        );
        // let replaced = first_line.replace(prefix_placeholder, target_prefix);
        destination.write_all(new_shebang.as_bytes())?;
        source_bytes = rest;
    }

    let mut last_match = 0;

    for index in memchr::memmem::find_iter(source_bytes, old_prefix) {
        destination.write_all(&source_bytes[last_match..index])?;
        destination.write_all(new_prefix)?;
        last_match = index + old_prefix.len();
    }

    // Write remaining bytes
    if last_match < source_bytes.len() {
        destination.write_all(&source_bytes[last_match..])?;
    }

    Ok(())
}

/// Writes `source[start..end]` to `destination`, returning an [`std::io::ErrorKind::InvalidData`]
/// error instead of panicking when the range is invalid (out of bounds or out of order).
///
/// The offsets driving the prefix replacement come from a package's `paths.json`, which is not
/// trusted input. A malformed or malicious entry (an offset past the end of the file, or offsets
/// that are not sorted/overlapping) must surface as a recoverable error rather than crash the
/// process (which for e.g. a FUSE/NFS mount would take down the whole mount).
fn write_replacement_range<W: Write>(
    destination: &mut W,
    source: &[u8],
    start: usize,
    end: usize,
) -> Result<(), std::io::Error> {
    let slice = source.get(start..end).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "invalid prefix replacement offsets: range {start}..{end} is out of bounds or out \
                 of order for content of length {}",
                source.len()
            ),
        )
    })?;
    destination.write_all(slice)
}

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text using the offsets from the `paths.json`.
///
/// This is a text based version where the complete string is replaced. This works fine for text
/// files but will not work correctly for binary files where the length of the string is often
/// important. See [`copy_and_replace_cstring_placeholder_offsets`] when you are dealing with binary
/// content.
///
/// `offsets` are absolute byte positions in `source_bytes` and, per the CEP, **exclude** any
/// occurrence inside the shebang region. Each listed offset is spliced uniformly. The shebang
/// region — the first `shebang_length` bytes, present exactly when the file starts with `#!` — is
/// handled separately: on targets with shebang handling ([`Platform::is_unix`]) the region minus
/// its trailing newline is rewritten by `replace_shebang` and the newline byte copied through
/// verbatim; on other targets the region gets plain placeholder replacement.
///
/// The recorded metadata is validated before anything is written, so a mismatch surfaces as
/// [`OffsetReplaceError::InconsistentMetadata`] with an untouched destination the caller can hand
/// to search-based replacement.
pub fn copy_and_replace_textual_placeholder_offsets(
    source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Platform,
    offsets: &[usize],
    shebang_length: Option<usize>,
) -> Result<(), OffsetReplaceError> {
    let old_prefix = prefix_placeholder.as_bytes();
    let new_prefix = target_prefix.as_bytes();

    // Determine the shebang region from the recorded `shebang_length` rather than re-deriving it
    // from the file contents. Per the CEP `shebang_length` is present exactly when the file starts
    // with `#!`, and its value is the offset of the first newline plus one (or the file size when
    // there is no newline). The first `shebang_length` bytes form the shebang region.
    let starts_with_shebang = source_bytes.starts_with(b"#!");
    let region_end = if starts_with_shebang {
        let len = shebang_length.ok_or_else(|| {
            OffsetReplaceError::inconsistent("file starts with #! but shebang_length is absent")
        })?;
        // Validate the recorded value against the actual contents.
        let expected = source_bytes
            .iter()
            .position(|&c| c == b'\n')
            .map_or(source_bytes.len(), |i| i + 1);
        if len != expected {
            return Err(OffsetReplaceError::inconsistent(format!(
                "shebang_length {len} does not match the first newline position + 1 ({expected})"
            )));
        }
        len
    } else {
        if shebang_length.is_some() {
            return Err(OffsetReplaceError::inconsistent(
                "shebang_length present but the file does not start with #!",
            ));
        }
        0
    };

    // Validate the offsets before writing anything so that, on inconsistent metadata, the caller
    // can fall back to search-based replacement using the still-empty destination. Offsets must be
    // in range, sorted in strictly increasing non-overlapping order, at or after the shebang
    // region, and the placeholder bytes must actually be present at each one.
    let mut prev_end = region_end;
    for &offset in offsets {
        if offset < region_end {
            return Err(OffsetReplaceError::inconsistent(format!(
                "offset {offset} lies inside the shebang region (< {region_end})"
            )));
        }
        if offset < prev_end {
            return Err(OffsetReplaceError::inconsistent(
                "offsets are not sorted in strictly increasing, non-overlapping order",
            ));
        }
        let end = offset
            .checked_add(old_prefix.len())
            .filter(|&end| end <= source_bytes.len())
            .ok_or_else(|| {
                OffsetReplaceError::inconsistent(format!(
                    "offset {offset} is out of range for content of length {}",
                    source_bytes.len()
                ))
            })?;
        if &source_bytes[offset..end] != old_prefix {
            return Err(OffsetReplaceError::inconsistent(format!(
                "placeholder bytes are not present at recorded offset {offset}"
            )));
        }
        prev_end = end;
    }

    // --- The metadata is consistent; write the patched file. ---

    // Handle the shebang region via the shared helper, so that install-time and
    // mount-time (rattler_vfs) replacement stay byte-identical.
    if region_end > 0 {
        let region_out = replace_shebang_region(
            &source_bytes[..region_end],
            prefix_placeholder,
            target_prefix,
            target_platform,
        );
        destination.write_all(&region_out)?;
    }

    // Splice the recorded body offsets.
    let mut last_match = region_end;
    for &offset in offsets {
        write_replacement_range(&mut destination, source_bytes, last_match, offset)?;
        destination.write_all(new_prefix)?;
        last_match = offset + old_prefix.len();
    }

    // Write any remaining bytes after the final replacement.
    if last_match < source_bytes.len() {
        destination.write_all(&source_bytes[last_match..])?;
    }

    Ok(())
}

/// Given the contents of a file, copies it to the `destination` and in the process replace any
/// binary c-style string that contains the text `prefix_placeholder` with a binary compatible
/// c-string where the `prefix_placeholder` text is replaced with the `target_prefix` text.
///
/// The length of the input will match the output.
///
/// This function replaces binary c-style strings. If you want to simply find-and-replace text in a
/// file instead use the [`copy_and_replace_textual_placeholder`] function.
pub fn copy_and_replace_cstring_placeholder(
    mut source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
) -> Result<(), std::io::Error> {
    // Get the prefixes as bytes
    let old_prefix = prefix_placeholder.as_bytes();
    let new_prefix = target_prefix.as_bytes();

    if new_prefix.len() > old_prefix.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "target prefix cannot be longer than the placeholder prefix",
        ));
    }

    let finder = memchr::memmem::Finder::new(old_prefix);

    loop {
        if let Some(index) = finder.find(source_bytes) {
            // write all bytes up to the old prefix, followed by the new prefix.
            destination.write_all(&source_bytes[..index])?;

            // Find the end of the c-style string. The null terminator basically.
            let mut end = index + old_prefix.len();
            while end < source_bytes.len() && source_bytes[end] != b'\0' {
                end += 1;
            }

            let mut out = Vec::new();
            let mut old_bytes = &source_bytes[index..end];
            let old_len = old_bytes.len();

            // replace all occurrences of the old prefix with the new prefix
            while let Some(index) = finder.find(old_bytes) {
                out.write_all(&old_bytes[..index])?;
                out.write_all(new_prefix)?;
                old_bytes = &old_bytes[index + old_prefix.len()..];
            }
            out.write_all(old_bytes)?;
            // write everything up to the old length
            if out.len() > old_len {
                destination.write_all(&out[..old_len])?;
            } else {
                destination.write_all(&out)?;
            }

            // Compute the padding required when replacing the old prefix(es) with the new one. If the old
            // prefix is longer than the new one we need to add padding to ensure that the entire part
            // will hold the same number of bytes. We do this by adding '\0's (e.g. null terminators). This
            // ensures that the text will remain a valid null-terminated string.
            let padding = old_len.saturating_sub(out.len());
            destination.write_all(&vec![0; padding])?;

            // Continue with the rest of the bytes.
            source_bytes = &source_bytes[end..];
        } else {
            // The old prefix was not found in the (remaining) source bytes.
            // Write the rest of the bytes
            destination.write_all(source_bytes)?;

            return Ok(());
        }
    }
}

/// Given the contents & offsets of a file, copies it to the `destination` and in the process replace
/// any binary c-style string that contains the text `prefix_placeholder` with a binary compatible
/// c-string where the `prefix_placeholder` text is replaced with the `target_prefix` text.
///
/// The length of the input will match the output.
///
/// Offsets are grouped by c-string: each inner slice lists the prefix start
/// positions followed by the position of the NUL terminator, or the file size
/// when the final c-string is unterminated at end-of-file (the padding then
/// runs to EOF, still preserving the length). For example, `[[5, 39], [22, 30,
/// 39]]` means one c-string with the prefix at offset 5 (NUL at 39), and
/// another with prefixes at 22 and 30 (NUL at 39).
///
/// The metadata is validated before anything is written, so a mismatch surfaces as
/// [`OffsetReplaceError::InconsistentMetadata`] with an untouched destination the caller can hand
/// to search-based replacement.
pub fn copy_and_replace_cstring_placeholder_offsets(
    source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    groups: &[Vec<usize>],
) -> Result<(), OffsetReplaceError> {
    let old_prefix = prefix_placeholder.as_bytes();
    let new_prefix = target_prefix.as_bytes();

    if new_prefix.len() > old_prefix.len() {
        return Err(OffsetReplaceError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "target prefix cannot be longer than the placeholder prefix",
        )));
    }

    // The binary form must list at least one c-string.
    if groups.is_empty() {
        return Err(OffsetReplaceError::inconsistent(
            "binary offsets outer list is empty",
        ));
    }

    // Validate every group before writing so that, on inconsistent metadata, the caller can fall
    // back to search-based replacement using the still-empty destination. Within each group the
    // prefix offsets must be in range (before the terminator), sorted in strictly increasing
    // non-overlapping order — also across groups — and the placeholder bytes must be present.
    let mut prev_end = 0usize;
    for group in groups {
        // Each group lists the prefix offsets followed by the NUL terminator position.
        let Some((&nul_pos, prefix_offsets)) = group.split_last() else {
            return Err(OffsetReplaceError::inconsistent(
                "binary offset group is empty",
            ));
        };
        if prefix_offsets.is_empty() {
            return Err(OffsetReplaceError::inconsistent(
                "binary offset group has no prefix offsets",
            ));
        }
        if nul_pos > source_bytes.len() {
            return Err(OffsetReplaceError::inconsistent(format!(
                "NUL offset {nul_pos} is out of range for content of length {}",
                source_bytes.len()
            )));
        }
        for &offset in prefix_offsets {
            if offset < prev_end {
                return Err(OffsetReplaceError::inconsistent(
                    "binary offsets are not sorted / c-string ranges overlap",
                ));
            }
            let end = offset
                .checked_add(old_prefix.len())
                .filter(|&end| end <= nul_pos)
                .ok_or_else(|| {
                    OffsetReplaceError::inconsistent(format!(
                        "offset {offset} does not fit before its NUL terminator {nul_pos}"
                    ))
                })?;
            if &source_bytes[offset..end] != old_prefix {
                return Err(OffsetReplaceError::inconsistent(format!(
                    "placeholder bytes are not present at recorded offset {offset}"
                )));
            }
            prev_end = end;
        }
        prev_end = nul_pos;
    }

    // --- The metadata is consistent; write the patched file. ---
    let length_change = old_prefix.len() - new_prefix.len();
    let mut last_pos = 0;

    for group in groups {
        // Validated above: non-empty group, terminator in range, offsets ordered and in range.
        let (&nul_pos, prefix_offsets) =
            group.split_last().expect("group validated to be non-empty");

        for &offset in prefix_offsets {
            // Write bytes between last position and this prefix
            write_replacement_range(&mut destination, source_bytes, last_pos, offset)?;
            // Write the new prefix
            destination.write_all(new_prefix)?;
            // Advance past old prefix in source
            last_pos = offset + old_prefix.len();
        }

        // Write remaining bytes from last prefix end to the NUL position (or EOF for an
        // unterminated final c-string, where `nul_pos == source_bytes.len()`).
        write_replacement_range(&mut destination, source_bytes, last_pos, nul_pos)?;

        // Pad with zeros to preserve total length. For an unterminated final c-string this padding
        // runs to the end of the file.
        let padding = prefix_offsets.len() * length_change;
        if padding > 0 {
            destination.write_all(&vec![0; padding])?;
        }

        last_pos = nul_pos;
    }

    // Write any remaining bytes after the last c-string
    if last_pos < source_bytes.len() {
        destination.write_all(&source_bytes[last_pos..])?;
    }

    Ok(())
}

fn symlink(source_path: &Path, destination_path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    return fs_err::os::windows::fs::symlink_file(source_path, destination_path);
    #[cfg(unix)]
    return fs_err::os::unix::fs::symlink(source_path, destination_path);
}

#[allow(unused_variables)]
fn has_executable_permissions(permissions: &Permissions) -> bool {
    #[cfg(windows)]
    return false;
    #[cfg(unix)]
    return std::os::unix::fs::PermissionsExt::mode(permissions) & 0o111 != 0;
}

/// Represents the type of file detected from its content
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FileType {
    /// A Mach-O binary (executable, dylib, bundle, etc.)
    MachO,
}

impl FileType {
    // Mach-O magic bytes constants
    const MACHO_FAT_MAGIC: u32 = 0xcafebabe; // Fat/Universal binary (big-endian)
    const MACHO_FAT_CIGAM: u32 = 0xbebafeca; // Fat/Universal binary (little-endian)
    const MACHO_MAGIC_32: u32 = 0xfeedface; // Mach-O 32-bit (big-endian)
    const MACHO_CIGAM_32: u32 = 0xcefaedfe; // Mach-O 32-bit (little-endian)
    const MACHO_MAGIC_64: u32 = 0xfeedfacf; // Mach-O 64-bit (big-endian)
    const MACHO_CIGAM_64: u32 = 0xcffaedfe; // Mach-O 64-bit (little-endian)

    /// Detects the file type by checking its magic bytes.
    /// Returns `Some(FileType)` if a known file type is detected, `None` otherwise.
    fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }

        let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);

        match magic {
            Self::MACHO_FAT_MAGIC
            | Self::MACHO_FAT_CIGAM
            | Self::MACHO_MAGIC_32
            | Self::MACHO_CIGAM_32
            | Self::MACHO_MAGIC_64
            | Self::MACHO_CIGAM_64 => Some(FileType::MachO),
            _ => None,
        }
    }
}

#[cfg(test)]
mod test {
    use super::ExternalSymlinkPolicy;
    use super::PYTHON_REGEX;
    use fs_err as fs;
    use rattler_conda_types::Platform;
    use rattler_conda_types::package::{OffsetEncoding, OffsetGroup, OffsetRanges};
    use rstest::rstest;
    use std::io::Cursor;

    /// Builds the UTF-8 offset group a CEP-conformant producer would emit.
    fn utf8_group(ranges: OffsetRanges) -> OffsetGroup {
        OffsetGroup {
            encoding: OffsetEncoding::Utf8,
            ranges,
            has_unknown_members: false,
        }
    }

    /// Patched files must receive `modification_time` rather than preserving
    /// the source file's mtime. Without this, Python reuses stale .pyc files
    /// whose headers record the original source mtime, even though the .py
    /// content was changed by prefix replacement.
    #[test]
    fn test_patched_file_receives_modification_time() {
        use super::AppleCodeSignBehavior;
        use rattler_conda_types::package::{FileMode, PathType, PathsEntry, PrefixPlaceholder};
        use rattler_conda_types::prefix::Prefix;
        use std::path::PathBuf;

        let temp_dir = tempfile::tempdir().unwrap();

        let package_dir = temp_dir.path().join("package");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("config.py"),
            "prefix = '/old/placeholder/path'\n",
        )
        .unwrap();

        let source_time = filetime::FileTime::from_unix_time(1_000_000, 0);
        filetime::set_file_times(package_dir.join("config.py"), source_time, source_time).unwrap();

        let target_dir = Prefix::create(temp_dir.path().join("target")).unwrap();
        let modification_time = filetime::FileTime::from_unix_time(2_000_000, 0);

        let entry = PathsEntry {
            relative_path: PathBuf::from("config.py"),
            no_link: false,
            path_type: PathType::HardLink,
            prefix_placeholder: Some(PrefixPlaceholder {
                file_mode: FileMode::Text,
                placeholder: "/old/placeholder/path".to_string(),
                experimental_offsets: None,
                experimental_shebang_length: None,
            }),
            sha256: None,
            size_in_bytes: None,
        };

        let result = super::link_file(
            &entry,
            PathBuf::from("config.py"),
            &package_dir,
            &target_dir,
            target_dir.path().to_str().unwrap(),
            true,
            true,
            true,
            Platform::Linux64,
            AppleCodeSignBehavior::DoNothing,
            modification_time,
            ExternalSymlinkPolicy::Deny,
        )
        .unwrap()
        .unwrap();

        assert_eq!(result.method, super::LinkMethod::Patched(FileMode::Text));

        let content = fs::read_to_string(target_dir.path().join("config.py")).unwrap();
        assert!(content.contains(target_dir.path().to_str().unwrap()));
        assert!(!content.contains("/old/placeholder/path"));

        let dest_metadata = fs::metadata(target_dir.path().join("config.py")).unwrap();
        let dest_mtime = filetime::FileTime::from_last_modification_time(&dest_metadata);
        assert_eq!(
            dest_mtime, modification_time,
            "patched file should have modification_time ({modification_time}), not source mtime ({source_time})",
        );
    }

    /// Files without `prefix_placeholder` are reflinked/hardlinked/copied and
    /// must keep their original mtime, not receive `modification_time`.
    #[test]
    fn test_unpatched_file_keeps_source_mtime() {
        use super::AppleCodeSignBehavior;
        use rattler_conda_types::package::{PathType, PathsEntry};
        use rattler_conda_types::prefix::Prefix;
        use std::path::PathBuf;

        let temp_dir = tempfile::tempdir().unwrap();

        let package_dir = temp_dir.path().join("package");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(package_dir.join("data.txt"), "no prefix here\n").unwrap();

        let source_time = filetime::FileTime::from_unix_time(1_000_000, 0);
        filetime::set_file_times(package_dir.join("data.txt"), source_time, source_time).unwrap();

        let target_dir = Prefix::create(temp_dir.path().join("target")).unwrap();
        let modification_time = filetime::FileTime::from_unix_time(2_000_000, 0);

        let entry = PathsEntry {
            relative_path: PathBuf::from("data.txt"),
            no_link: false,
            path_type: PathType::HardLink,
            prefix_placeholder: None,
            sha256: None,
            size_in_bytes: None,
        };

        let result = super::link_file(
            &entry,
            PathBuf::from("data.txt"),
            &package_dir,
            &target_dir,
            target_dir.path().to_str().unwrap(),
            true,
            true,
            true,
            Platform::Linux64,
            AppleCodeSignBehavior::DoNothing,
            modification_time,
            ExternalSymlinkPolicy::Deny,
        )
        .unwrap()
        .unwrap();

        assert_ne!(
            result.method,
            super::LinkMethod::Patched(rattler_conda_types::package::FileMode::Text)
        );
        assert_ne!(
            result.method,
            super::LinkMethod::Patched(rattler_conda_types::package::FileMode::Binary)
        );

        let dest_metadata = fs::metadata(target_dir.path().join("data.txt")).unwrap();
        let dest_mtime = filetime::FileTime::from_last_modification_time(&dest_metadata);
        assert_eq!(
            dest_mtime, source_time,
            "unpatched file should keep source mtime ({source_time}), not modification_time ({modification_time})",
        );
    }

    /// A `SoftLink` entry whose source is missing on disk (because extraction
    /// skipped it, e.g. on Windows without symlink privileges) should be
    /// skipped with `Ok(None)` instead of failing the whole install.
    #[test]
    fn test_missing_symlink_source_is_skipped() {
        use super::AppleCodeSignBehavior;
        use rattler_conda_types::package::{PathType, PathsEntry};
        use rattler_conda_types::prefix::Prefix;
        use std::path::PathBuf;

        let temp_dir = tempfile::tempdir().unwrap();

        let package_dir = temp_dir.path().join("package");
        fs::create_dir_all(&package_dir).unwrap();
        // Intentionally do NOT create `missing-link` in package_dir.

        let target_dir = Prefix::create(temp_dir.path().join("target")).unwrap();
        let modification_time = filetime::FileTime::from_unix_time(2_000_000, 0);

        let entry = PathsEntry {
            relative_path: PathBuf::from("missing-link"),
            no_link: false,
            path_type: PathType::SoftLink,
            prefix_placeholder: None,
            sha256: None,
            size_in_bytes: None,
        };

        let result = super::link_file(
            &entry,
            PathBuf::from("missing-link"),
            &package_dir,
            &target_dir,
            target_dir.path().to_str().unwrap(),
            true,
            true,
            true,
            Platform::Linux64,
            AppleCodeSignBehavior::DoNothing,
            modification_time,
            ExternalSymlinkPolicy::Deny,
        )
        .unwrap();

        assert!(
            result.is_none(),
            "expected Ok(None) for missing symlink source"
        );
        assert!(
            !target_dir.path().join("missing-link").exists(),
            "no destination file should have been created"
        );
    }

    #[rstest]
    #[case("Hello, cruel world!", "cruel", "fabulous", "Hello, fabulous world!")]
    #[case(
        "prefix_placeholder",
        "prefix_placeholder",
        "target_prefix",
        "target_prefix"
    )]
    pub fn test_copy_and_replace_textual_placeholder(
        #[case] input: &str,
        #[case] prefix_placeholder: &str,
        #[case] target_prefix: &str,
        #[case] expected_output: &str,
    ) {
        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder(
            input.as_bytes(),
            &mut output,
            prefix_placeholder,
            target_prefix,
            &Platform::Linux64,
        )
        .unwrap();
        assert_eq!(
            &String::from_utf8_lossy(&output.into_inner()),
            expected_output
        );
    }

    #[rstest]
    #[case(
        b"12345Hello, fabulous world!\x006789",
        "fabulous",
        "cruel",
        b"12345Hello, cruel world!\x00\x00\x00\x006789"
    )]
    pub fn test_copy_and_replace_binary_placeholder(
        #[case] input: &[u8],
        #[case] prefix_placeholder: &str,
        #[case] target_prefix: &str,
        #[case] expected_output: &[u8],
    ) {
        assert_eq!(
            expected_output.len(),
            input.len(),
            "input and expected output must have the same length"
        );
        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_cstring_placeholder(
            input,
            &mut output,
            prefix_placeholder,
            target_prefix,
        )
        .unwrap();
        assert_eq!(&output.into_inner(), expected_output);
    }

    #[rstest]
    #[case(b"short\x00", "short", "verylong")]
    #[case(b"short1234\x00", "short", "verylong")]
    pub fn test_shorter_binary_placeholder(
        #[case] input: &[u8],
        #[case] prefix_placeholder: &str,
        #[case] target_prefix: &str,
    ) {
        assert!(target_prefix.len() > prefix_placeholder.len());

        let mut output = Cursor::new(Vec::new());
        let result = super::copy_and_replace_cstring_placeholder(
            input,
            &mut output,
            prefix_placeholder,
            target_prefix,
        );
        assert!(result.is_err());
    }

    #[test]
    fn replace_binary_path_var() {
        let input =
            b"beginrandomdataPATH=/placeholder/etc/share:/placeholder/bin/:\x00somemoretext";
        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_cstring_placeholder(input, &mut output, "/placeholder", "/target")
            .unwrap();
        let out = &output.into_inner();
        assert_eq!(out, b"beginrandomdataPATH=/target/etc/share:/target/bin/:\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00somemoretext");
        assert_eq!(out.len(), input.len());
    }

    #[test]
    fn test_replace_shebang() {
        let shebang_with_spaces = "#!/path/placeholder/executable -o test -x".into();
        let replaced = super::replace_shebang(
            shebang_with_spaces,
            ("placeholder", "with space"),
            &Platform::Linux64,
        );
        assert_eq!(replaced, "#!/usr/bin/env executable -o test -x");
    }

    #[test]
    fn test_replace_long_shebang() {
        let short_shebang = "#!/path/to/executable -x 123".into();
        let replaced = super::replace_shebang(short_shebang, ("", ""), &Platform::Linux64);
        assert_eq!(replaced, "#!/path/to/executable -x 123");

        let shebang = "#!/this/is/loooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooong/executable -o test -x";
        let replaced = super::replace_shebang(shebang.into(), ("", ""), &Platform::Linux64);
        assert_eq!(replaced, "#!/usr/bin/env executable -o test -x");

        let replaced = super::replace_shebang(shebang.into(), ("", ""), &Platform::Osx64);
        assert_eq!(replaced, shebang);

        let shebang_with_escapes = "#!/this/is/loooooooooooooooooooooooooooooooooooooooooooooooooooo\\ oooooo\\ oooooo\\ oooooooooooooooooooooooooooooooooooong/exe\\ cutable -o test -x";
        let replaced =
            super::replace_shebang(shebang_with_escapes.into(), ("", ""), &Platform::Linux64);
        assert_eq!(replaced, "#!/usr/bin/env exe\\ cutable -o test -x");

        let shebang = "#!    /this/is/looooooooooooooooooooooooooooooooooooooooooooo\\ \\ ooooooo\\ oooooo\\ oooooo\\ ooooooooooooooooo\\ ooooooooooooooooooong/exe\\ cutable -o \"te  st\" -x";
        let replaced = super::replace_shebang(shebang.into(), ("", ""), &Platform::Linux64);
        assert_eq!(replaced, "#!/usr/bin/env exe\\ cutable -o \"te  st\" -x");

        let shebang = "#!/usr/bin/env perl";
        let replaced = super::replace_shebang(
            shebang.into(),
            ("/placeholder", "/with space"),
            &Platform::Linux64,
        );
        assert_eq!(replaced, shebang);

        let shebang = "#!/placeholder/perl";
        let replaced = super::replace_shebang(
            shebang.into(),
            ("/placeholder", "/with space"),
            &Platform::Linux64,
        );
        assert_eq!(replaced, "#!/usr/bin/env perl");
    }

    #[test]
    fn replace_python_shebang() {
        let short_shebang = "#!/path/to/python3.12".into();
        let replaced = super::replace_shebang(
            short_shebang,
            ("/path/to", "/new/prefix/with spaces/bin"),
            &Platform::Linux64,
        );
        insta::assert_snapshot!(replaced);

        let short_shebang = "#!/path/to/python3.12 -x 123".into();
        let replaced = super::replace_shebang(
            short_shebang,
            ("/path/to", "/new/prefix/with spaces/bin"),
            &Platform::Linux64,
        );
        insta::assert_snapshot!(replaced);
    }

    #[test]
    fn test_replace_long_prefix_in_text_file() {
        let test_data_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
        let test_file = test_data_dir.join("shebang_test.txt");
        let prefix_placeholder = "/this/is/placeholder";
        let mut target_prefix = "/super/long/".to_string();
        for _ in 0..15 {
            target_prefix.push_str("verylongstring/");
        }
        let input = fs::read(test_file).unwrap();
        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder(
            &input,
            &mut output,
            prefix_placeholder,
            &target_prefix,
            &Platform::Linux64,
        )
        .unwrap();

        let output = output.into_inner();
        let replaced = String::from_utf8_lossy(&output);
        insta::assert_snapshot!(replaced);
    }

    #[test]
    fn test_python_regex() {
        // Test the regex
        let test_strings = vec!["python", "python3", "python3.12", "python2.7"];

        for s in test_strings {
            assert!(PYTHON_REGEX.is_match(s));
        }

        let no_match_strings = vec![
            "python3.12.1",
            "python3.12.1.1",
            "foo",
            "foo3.2",
            "pythondoc",
        ];

        for s in no_match_strings {
            assert!(!PYTHON_REGEX.is_match(s));
        }
    }

    #[test]
    fn test_detect_file_type() {
        use super::FileType;

        // Test Mach-O 64-bit magic (big-endian)
        let macho_64_be = [0xfe, 0xed, 0xfa, 0xcf, 0x00, 0x00];
        assert_eq!(FileType::detect(&macho_64_be), Some(FileType::MachO));

        // Test Mach-O 64-bit magic (little-endian)
        let macho_64_le = [0xcf, 0xfa, 0xed, 0xfe, 0x00, 0x00];
        assert_eq!(FileType::detect(&macho_64_le), Some(FileType::MachO));

        // Test Mach-O 32-bit magic (big-endian)
        let macho_32_be = [0xfe, 0xed, 0xfa, 0xce, 0x00, 0x00];
        assert_eq!(FileType::detect(&macho_32_be), Some(FileType::MachO));

        // Test Mach-O 32-bit magic (little-endian)
        let macho_32_le = [0xce, 0xfa, 0xed, 0xfe, 0x00, 0x00];
        assert_eq!(FileType::detect(&macho_32_le), Some(FileType::MachO));

        // Test Fat/Universal binary magic (big-endian)
        let fat_be = [0xca, 0xfe, 0xba, 0xbe, 0x00, 0x00];
        assert_eq!(FileType::detect(&fat_be), Some(FileType::MachO));

        // Test Fat/Universal binary magic (little-endian)
        let fat_le = [0xbe, 0xba, 0xfe, 0xca, 0x00, 0x00];
        assert_eq!(FileType::detect(&fat_le), Some(FileType::MachO));

        // Test non-Mach-O file
        let not_macho = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05];
        assert_eq!(FileType::detect(&not_macho), None);

        // Test short file
        let short = [0xfe, 0xed];
        assert_eq!(FileType::detect(&short), None);

        // Test empty file
        let empty: [u8; 0] = [];
        assert_eq!(FileType::detect(&empty), None);
    }

    #[test]
    fn test_symlink_escape_rejected() {
        use super::{LinkFileError, symlink_to_destination};

        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("prefix");
        let cache = tmp.path().join("cache");
        fs::create_dir_all(prefix.join("lib")).unwrap();
        fs::create_dir_all(cache.join("lib")).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("../../../../escape_target", cache.join("lib/sneaky-link"))
            .unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(
            "..\\..\\..\\..\\escape_target",
            cache.join("lib\\sneaky-link"),
        )
        .unwrap();

        let result = symlink_to_destination(
            &cache.join("lib/sneaky-link"),
            &prefix.join("lib/sneaky-link"),
            &prefix,
            ExternalSymlinkPolicy::Deny,
        );
        assert!(matches!(
            result.unwrap_err(),
            LinkFileError::SymlinkTargetEscapesPrefix
        ));
    }

    #[test]
    fn test_symlink_within_prefix_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("prefix");
        let cache = tmp.path().join("cache");
        fs::create_dir_all(prefix.join("lib")).unwrap();
        fs::create_dir_all(cache.join("lib")).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("../bin/real_file", cache.join("lib/safe-link")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file("..\\bin\\real_file", cache.join("lib\\safe-link"))
            .unwrap();

        let result = super::symlink_to_destination(
            &cache.join("lib/safe-link"),
            &prefix.join("lib/safe-link"),
            &prefix,
            ExternalSymlinkPolicy::Deny,
        );
        assert!(result.is_ok());
    }

    #[cfg_attr(
        windows,
        ignore = "creating symlinks on Windows requires elevated privileges"
    )]
    #[test]
    fn test_copy_symlink_target_to_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let dest_dir = tmp.path().join("dest");
        fs::create_dir_all(cache.join("bin")).unwrap();
        fs::create_dir_all(cache.join("lib")).unwrap();
        fs::create_dir_all(&dest_dir).unwrap();

        let real_file = cache.join("bin/real_file");
        fs::write(&real_file, b"hello world").unwrap();

        let symlink_path = cache.join("lib/link");
        #[cfg(unix)]
        std::os::unix::fs::symlink("../bin/real_file", &symlink_path).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file("..\\bin\\real_file", &symlink_path).unwrap();

        let dest_path = dest_dir.join("link");
        let method = super::copy_symlink_target_to_destination(&symlink_path, &dest_path)
            .expect("copying through a symlink source should succeed");

        assert_eq!(method, super::LinkMethod::Copy);
        assert!(
            !dest_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink(),
            "destination should be a regular file, not a symlink"
        );
        assert_eq!(fs::read(&dest_path).unwrap(), b"hello world");
    }

    #[rstest]
    #[case("Hello, cruel world!", [7].to_vec(), "cruel", "fabulous", "Hello, fabulous world!")]
    #[case(
        "prefix_placeholder",
        [0].to_vec(),
        "prefix_placeholder",
        "target_prefix",
        "target_prefix"
    )]
    pub fn test_copy_and_replace_textual_placeholder_with_offsets(
        #[case] input: &str,
        #[case] offsets: Vec<usize>,
        #[case] prefix_placeholder: &str,
        #[case] target_prefix: &str,
        #[case] expected_output: &str,
    ) {
        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder_offsets(
            input.as_bytes(),
            &mut output,
            prefix_placeholder,
            target_prefix,
            &Platform::Linux64,
            &offsets,
            None,
        )
        .unwrap();
        assert_eq!(
            &String::from_utf8_lossy(&output.into_inner()),
            expected_output
        );
    }

    /// Records only the body occurrences in `offsets`, filtering out the ones inside the shebang
    /// region — exactly what a CEP-conformant producer emits.
    fn conformant_text_offsets(input: &[u8], placeholder: &str) -> (Vec<usize>, Option<usize>) {
        let shebang_length = input.starts_with(b"#!").then(|| {
            input
                .iter()
                .position(|&c| c == b'\n')
                .map_or(input.len(), |i| i + 1)
        });
        let region_end = shebang_length.unwrap_or(0);
        let offsets = memchr::memmem::find_iter(input, placeholder.as_bytes())
            .filter(|&o| o >= region_end)
            .collect();
        (offsets, shebang_length)
    }

    /// CEP test vector 1: a Unix target with a short target prefix. The occurrence inside the
    /// shebang line (excluded from `offsets`) is rewritten by the shebang rules and, being short
    /// enough, the patched line is kept; the body occurrence is spliced at its recorded offset.
    #[test]
    fn test_textual_offsets_shebang_kept_short_prefix() {
        let prefix_placeholder = "/this/is/placeholder";
        let target_prefix = "/opt/conda";
        let input =
            format!("#!{prefix_placeholder}/python\nimport sys  # see {prefix_placeholder}/lib\n")
                .into_bytes();

        let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
        assert_eq!(offsets.len(), 1, "only the body occurrence is recorded");
        assert_eq!(shebang_length, Some(30));

        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder_offsets(
            &input,
            &mut output,
            prefix_placeholder,
            target_prefix,
            &Platform::Linux64,
            &offsets,
            shebang_length,
        )
        .unwrap();

        let expected = format!("#!{target_prefix}/python\nimport sys  # see {target_prefix}/lib\n");
        assert_eq!(String::from_utf8_lossy(&output.into_inner()), expected);
    }

    /// CEP test vector 3: a non-rewriting target (Windows, e.g. a `noarch` package) with an
    /// occurrence inside the shebang region. There is no shebang machinery, so the region MUST get
    /// plain placeholder replacement even though its occurrence is not in `offsets`.
    #[test]
    fn test_textual_offsets_shebang_windows_plain_region() {
        let prefix_placeholder = "/this/is/placeholder";
        let target_prefix = "/opt/conda";
        let input =
            format!("#!{prefix_placeholder}/python\nimport sys  # see {prefix_placeholder}/lib\n")
                .into_bytes();

        let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);

        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder_offsets(
            &input,
            &mut output,
            prefix_placeholder,
            target_prefix,
            &Platform::Win64,
            &offsets,
            shebang_length,
        )
        .unwrap();

        // The shebang region's occurrence is replaced by the plain in-region path, not left behind.
        let expected = format!("#!{target_prefix}/python\nimport sys  # see {target_prefix}/lib\n");
        assert_eq!(String::from_utf8_lossy(&output.into_inner()), expected);
    }

    /// CEP test vector 4: a shebang file with no trailing newline, where `shebang_length` equals
    /// the file size. The whole file is the shebang line and there is no newline to copy through.
    #[test]
    fn test_textual_offsets_shebang_no_trailing_newline() {
        let prefix_placeholder = "/this/is/placeholder";
        let target_prefix = "/opt/conda";
        let input = format!("#!{prefix_placeholder}/python").into_bytes();

        let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
        assert!(offsets.is_empty(), "the only occurrence is in the shebang");
        assert_eq!(shebang_length, Some(input.len()));

        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder_offsets(
            &input,
            &mut output,
            prefix_placeholder,
            target_prefix,
            &Platform::Linux64,
            &offsets,
            shebang_length,
        )
        .unwrap();

        assert_eq!(
            String::from_utf8_lossy(&output.into_inner()),
            format!("#!{target_prefix}/python")
        );
    }

    /// CEP test vector 5: a file whose only occurrence is in the shebang line, so `offsets` is the
    /// empty list. The shebang is short enough to keep.
    #[test]
    fn test_textual_offsets_only_shebang_occurrence_empty_offsets() {
        let prefix_placeholder = "/this/is/placeholder";
        let target_prefix = "/opt/conda";
        let input = format!("#!{prefix_placeholder}/python\nimport sys\n").into_bytes();

        let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
        assert!(offsets.is_empty());

        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder_offsets(
            &input,
            &mut output,
            prefix_placeholder,
            target_prefix,
            &Platform::Linux64,
            &offsets,
            shebang_length,
        )
        .unwrap();

        assert_eq!(
            String::from_utf8_lossy(&output.into_inner()),
            format!("#!{target_prefix}/python\nimport sys\n")
        );
    }

    /// CEP test vector 6: multiple occurrences within one shebang line. All of them are in the
    /// region (so `offsets` is empty) and the shebang rules replace them all.
    #[test]
    fn test_textual_offsets_multiple_occurrences_in_shebang() {
        let prefix_placeholder = "/this/is/placeholder";
        let target_prefix = "/opt/conda";
        let input =
            format!("#!{prefix_placeholder}/python -S {prefix_placeholder}/site\nprint(1)\n")
                .into_bytes();

        let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
        assert!(
            offsets.is_empty(),
            "both occurrences are inside the shebang line"
        );

        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder_offsets(
            &input,
            &mut output,
            prefix_placeholder,
            target_prefix,
            &Platform::Linux64,
            &offsets,
            shebang_length,
        )
        .unwrap();

        assert_eq!(
            String::from_utf8_lossy(&output.into_inner()),
            format!("#!{target_prefix}/python -S {target_prefix}/site\nprint(1)\n")
        );
    }

    /// CEP test vector 7: a shebang line longer than the kernel limit that contains no occurrence
    /// of the placeholder. `shebang_length` is still present (the file starts with `#!`) and the
    /// over-long line collapses to the `#!/usr/bin/env <program>` form regardless.
    #[test]
    fn test_textual_offsets_overlong_shebang_no_occurrence() {
        let prefix_placeholder = "/this/is/placeholder";
        let target_prefix = "/opt/conda";
        let long_shebang = "#!/this/is/loooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooong/executable -o test -x";
        assert!(long_shebang.len() > 127);
        let input = format!("{long_shebang}\nprint(1)\n").into_bytes();

        let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
        assert!(offsets.is_empty(), "the placeholder does not occur at all");
        assert_eq!(shebang_length, Some(long_shebang.len() + 1));

        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder_offsets(
            &input,
            &mut output,
            prefix_placeholder,
            target_prefix,
            &Platform::Linux64,
            &offsets,
            shebang_length,
        )
        .unwrap();

        assert_eq!(
            String::from_utf8_lossy(&output.into_inner()),
            "#!/usr/bin/env executable -o test -x\nprint(1)\n"
        );
    }

    /// A CEP-conformant producer never lists a shebang-region occurrence in `offsets`. If a
    /// non-conformant producer does, the offset function reports inconsistent metadata (writing
    /// nothing) so the installer falls back to search-based replacement — which produces the same
    /// bytes.
    #[test]
    fn test_textual_offsets_shebang_occurrence_in_offsets_is_inconsistent() {
        let prefix_placeholder = "/this/is/placeholder";
        let target_prefix = "/opt/conda";
        let input =
            format!("#!{prefix_placeholder}/python\nimport sys  # see {prefix_placeholder}/lib\n")
                .into_bytes();
        let shebang_length = input.iter().position(|&c| c == b'\n').unwrap() + 1;

        // Non-conformant: lists BOTH occurrences, including the in-region one at offset 2.
        let non_conformant: Vec<usize> =
            memchr::memmem::find_iter(&input, prefix_placeholder.as_bytes()).collect();
        assert_eq!(non_conformant.len(), 2);

        let mut output = Cursor::new(Vec::new());
        let result = super::copy_and_replace_textual_placeholder_offsets(
            &input,
            &mut output,
            prefix_placeholder,
            target_prefix,
            &Platform::Linux64,
            &non_conformant,
            Some(shebang_length),
        );
        assert!(
            matches!(
                result,
                Err(super::OffsetReplaceError::InconsistentMetadata(_))
            ),
            "in-region offset must be rejected: {result:?}"
        );
        assert!(
            output.into_inner().is_empty(),
            "nothing is written, so the fallback starts from a clean destination"
        );

        // The search-based fallback produces the correct bytes.
        let mut fallback = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder(
            &input,
            &mut fallback,
            prefix_placeholder,
            target_prefix,
            &Platform::Linux64,
        )
        .unwrap();
        let expected = format!("#!{target_prefix}/python\nimport sys  # see {target_prefix}/lib\n");
        assert_eq!(String::from_utf8_lossy(&fallback.into_inner()), expected);
    }

    /// A file that starts with `#!` but carries no `shebang_length` is producer non-conformance and
    /// must be reported as inconsistent metadata rather than mishandled.
    #[test]
    fn test_textual_offsets_shebang_length_absent_is_inconsistent() {
        let mut output = Cursor::new(Vec::new());
        let result = super::copy_and_replace_textual_placeholder_offsets(
            b"#!/this/is/placeholder/python\n",
            &mut output,
            "/this/is/placeholder",
            "/opt/conda",
            &Platform::Linux64,
            &[],
            None,
        );
        assert!(
            matches!(
                result,
                Err(super::OffsetReplaceError::InconsistentMetadata(_))
            ),
            "{result:?}"
        );
        assert!(output.into_inner().is_empty());
    }

    /// A `shebang_length` that disagrees with the first-newline position is inconsistent.
    #[test]
    fn test_textual_offsets_shebang_length_mismatch_is_inconsistent() {
        let mut output = Cursor::new(Vec::new());
        let result = super::copy_and_replace_textual_placeholder_offsets(
            b"#!/this/is/placeholder/python\nbody\n",
            &mut output,
            "/this/is/placeholder",
            "/opt/conda",
            &Platform::Linux64,
            &[],
            Some(20), // the correct value is 30
        );
        assert!(
            matches!(
                result,
                Err(super::OffsetReplaceError::InconsistentMetadata(_))
            ),
            "{result:?}"
        );
        assert!(output.into_inner().is_empty());
    }

    /// Regression for the search-based path: a `#!` file with no trailing newline must not panic.
    /// Previously the missing newline yielded an empty "first line", tripping an assertion inside
    /// `replace_shebang`.
    #[test]
    fn test_scan_path_shebang_without_newline_does_not_panic() {
        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder(
            b"#!/this/is/placeholder/python",
            &mut output,
            "/this/is/placeholder",
            "/opt/conda",
            &Platform::Linux64,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.into_inner()),
            "#!/opt/conda/python"
        );
    }

    /// CEP test vector 8: a binary file whose final C string is unterminated at end-of-file. The
    /// group's last value is the file size and the length-preserving padding runs to EOF.
    #[test]
    fn test_binary_offsets_unterminated_final_cstring() {
        let input = b"AAAA/placeholder";
        let groups = vec![vec![4, input.len()]];

        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_cstring_placeholder_offsets(
            input,
            &mut output,
            "/placeholder",
            "/opt",
            &groups,
        )
        .unwrap();

        let out = output.into_inner();
        assert_eq!(out, b"AAAA/opt\0\0\0\0\0\0\0\0");
        assert_eq!(out.len(), input.len(), "length must be preserved");
    }

    /// The dispatcher applies the UTF-8 group's ranges for a text file.
    #[test]
    fn test_offset_groups_text_utf8_group_applied() {
        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_placeholders_with_offsets(
            b"Hello, cruel world!",
            &mut output,
            "cruel",
            "fabulous",
            &Platform::Linux64,
            super::FileMode::Text,
            &[utf8_group(OffsetRanges::Text(vec![7]))],
            None,
        )
        .unwrap();
        assert_eq!(output.into_inner(), b"Hello, fabulous world!");
    }

    /// CEP test vector 9: a binary file with occurrences under more than one
    /// encoding. rattler's search-based replacement covers UTF-8 only, so the
    /// UTF-8 group is spliced and the UTF-16-LE wide string is left untouched
    /// — exactly as rattler's own search would leave it. The file length is
    /// preserved either way.
    #[test]
    fn test_offset_groups_binary_multi_encoding_vector9() {
        let placeholder = "/pfx";
        let target = "/np";

        // A UTF-8 c-string with the placeholder at offset 1 (NUL at 9),
        // followed by a UTF-16-LE wide string with the placeholder at offset
        // 10 (two-byte NUL terminator starting at 28), followed by a tail.
        let wide: Vec<u8> = "/pfx/wide"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut input = b"A/pfx/lib\0".to_vec();
        assert_eq!(input.len(), 10);
        input.extend_from_slice(&wide);
        input.extend_from_slice(&[0, 0]);
        input.extend_from_slice(b"tail");

        let groups = [
            OffsetGroup {
                encoding: OffsetEncoding::Utf16Le,
                ranges: OffsetRanges::Binary(vec![vec![10, 28]]),
                has_unknown_members: false,
            },
            utf8_group(OffsetRanges::Binary(vec![vec![1, 9]])),
        ];

        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_placeholders_with_offsets(
            &input,
            &mut output,
            placeholder,
            target,
            &Platform::Linux64,
            super::FileMode::Binary,
            &groups,
            None,
        )
        .unwrap();

        let out = output.into_inner();
        assert_eq!(out.len(), input.len(), "length must be preserved");
        // The UTF-8 c-string is patched, with padding restoring its length.
        assert_eq!(&out[..10], b"A/np/lib\0\0");
        // Everything from the wide string onwards is byte-identical.
        assert_eq!(&out[10..], &input[10..], "wide string must be untouched");
    }

    /// Valid metadata whose only groups are wide-string encodings records no
    /// UTF-8 occurrences: the file is copied through unchanged rather than
    /// treated as inconsistent.
    #[test]
    fn test_offset_groups_without_utf8_group_copies_verbatim() {
        let input = b"no utf-8 occurrences here";
        for (file_mode, ranges) in [
            (
                super::FileMode::Binary,
                OffsetRanges::Binary(vec![vec![10, 28]]),
            ),
            (super::FileMode::Text, OffsetRanges::Text(vec![10])),
        ] {
            let groups = [OffsetGroup {
                encoding: OffsetEncoding::Utf16Le,
                ranges,
                has_unknown_members: false,
            }];
            let mut output = Cursor::new(Vec::new());
            super::copy_and_replace_placeholders_with_offsets(
                input,
                &mut output,
                "/pfx",
                "/np",
                &Platform::Linux64,
                file_mode,
                &groups,
                None,
            )
            .unwrap();
            assert_eq!(output.into_inner(), input, "mode {file_mode:?}");
        }
    }

    /// Structurally invalid group lists — an unrecognized encoding, duplicate
    /// encodings, or an empty list for a binary file — surface as inconsistent
    /// metadata (with nothing written) so the installer falls back to the
    /// search-based replacement.
    #[rstest]
    #[case::unknown_encoding(vec![OffsetGroup {
        encoding: OffsetEncoding::Unknown(String::from("utf-64-xe")),
        ranges: OffsetRanges::Binary(vec![vec![1, 9]]),
        has_unknown_members: false,
    }])]
    #[case::duplicate_encoding(vec![
        utf8_group(OffsetRanges::Binary(vec![vec![1, 9]])),
        utf8_group(OffsetRanges::Binary(vec![vec![1, 9]])),
    ])]
    #[case::empty_list(vec![])]
    fn test_offset_groups_invalid_is_inconsistent(#[case] groups: Vec<OffsetGroup>) {
        let mut output = Cursor::new(Vec::new());
        let result = super::copy_and_replace_placeholders_with_offsets(
            b"A/pfx/lib\0",
            &mut output,
            "/pfx",
            "/np",
            &Platform::Linux64,
            super::FileMode::Binary,
            &groups,
            None,
        );
        assert!(
            matches!(
                result,
                Err(super::OffsetReplaceError::InconsistentMetadata(_))
            ),
            "{result:?}"
        );
        assert!(
            output.into_inner().is_empty(),
            "nothing is written, so the fallback starts from a clean destination"
        );
    }

    /// Offsets come from the (untrusted) `paths.json`. Malformed offsets must return a recoverable
    /// error rather than panic and take down the caller (e.g. a FUSE/NFS read thread).
    #[rstest]
    // Offset past the end of the file.
    #[case(vec![1000])]
    // Offsets out of order (second starts before the first prefix ends).
    #[case(vec![7, 0])]
    fn test_textual_offsets_invalid_returns_error(#[case] offsets: Vec<usize>) {
        let mut output = Cursor::new(Vec::new());
        let result = super::copy_and_replace_textual_placeholder_offsets(
            b"Hello, cruel world!",
            &mut output,
            "cruel",
            "fabulous",
            &Platform::Linux64,
            &offsets,
            None,
        );
        assert!(
            matches!(
                result,
                Err(super::OffsetReplaceError::InconsistentMetadata(_))
            ),
            "malformed offsets should surface as inconsistent metadata, not a panic: {result:?}"
        );
        // Nothing must be written when the metadata is rejected, so the caller can reuse the
        // destination for search-based replacement.
        assert!(output.into_inner().is_empty());
    }

    /// Malformed binary offset groups must also return an error rather than panic (empty group,
    /// out-of-range NUL position, ...).
    #[rstest]
    // Empty group would underflow `group.len() - 1`.
    #[case(vec![vec![]])]
    // Prefix offset and NUL position beyond the end of the file.
    #[case(vec![vec![1000, 2000]])]
    fn test_binary_offsets_invalid_returns_error(#[case] groups: Vec<Vec<usize>>) {
        let mut output = Cursor::new(Vec::new());
        let result = super::copy_and_replace_cstring_placeholder_offsets(
            b"12345Hello, fabulous world!\x006789",
            &mut output,
            "fabulous",
            "cruel",
            &groups,
        );
        assert!(
            matches!(
                result,
                Err(super::OffsetReplaceError::InconsistentMetadata(_))
            ),
            "malformed offsets should surface as inconsistent metadata, not a panic: {result:?}"
        );
        // Nothing must be written when the metadata is rejected.
        assert!(output.into_inner().is_empty());
    }

    #[rstest]
    // The NUL terminator sits at offset 27 (the `\x00` byte), not 28. The last value of the group
    // is the NUL position, per the CEP.
    #[case(
        b"12345Hello, fabulous world!\x006789",
        vec![vec![12, 27]],
        "fabulous",
        "cruel",
        b"12345Hello, cruel world!\x00\x00\x00\x006789"
    )]
    pub fn test_copy_and_replace_binary_placeholder_offsets(
        #[case] input: &[u8],
        #[case] groups: Vec<Vec<usize>>,
        #[case] prefix_placeholder: &str,
        #[case] target_prefix: &str,
        #[case] expected_output: &[u8],
    ) {
        assert_eq!(
            expected_output.len(),
            input.len(),
            "input and expected output must have the same length"
        );
        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_cstring_placeholder_offsets(
            input,
            &mut output,
            prefix_placeholder,
            target_prefix,
            &groups,
        )
        .unwrap();
        assert_eq!(&output.into_inner(), expected_output);
    }

    #[rstest]
    #[case(b"short\x00", vec![vec![0, 5]], "short", "verylong")]
    #[case(b"short1234\x00", vec![vec![0, 9]], "short", "verylong")]
    pub fn test_shorter_binary_placeholder_offsets(
        #[case] input: &[u8],
        #[case] groups: Vec<Vec<usize>>,
        #[case] prefix_placeholder: &str,
        #[case] target_prefix: &str,
    ) {
        assert!(target_prefix.len() > prefix_placeholder.len());

        let mut output = Cursor::new(Vec::new());
        let result = super::copy_and_replace_cstring_placeholder_offsets(
            input,
            &mut output,
            prefix_placeholder,
            target_prefix,
            &groups,
        );
        assert!(result.is_err());
    }

    #[rstest]
    #[case(
        b"beginrandomdataPATH=/placeholder/etc/share:/placeholder/bin/:\x00somemoretext",
        b"beginrandomdataPATH=/target/etc/share:/target/bin/:\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00somemoretext",
        vec![vec![20, 43, 61]]
    )]
    #[case(
        b"beginrandomdataPATH=/placeholder/etc/share:/placeholder/bin/another/placeholder/:\x00somemoretext",
        b"beginrandomdataPATH=/target/etc/share:/target/bin/another/target/:\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00somemoretext",
        vec![vec![20, 43, 67, 81]],
    )]
    fn replace_binary_path_var_offsets(
        #[case] input: &[u8],
        #[case] result: &[u8],
        #[case] groups: Vec<Vec<usize>>,
    ) {
        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_cstring_placeholder_offsets(
            input,
            &mut output,
            "/placeholder",
            "/target",
            &groups,
        )
        .unwrap();
        let out = &output.into_inner();
        assert_eq!(out, result);
        assert_eq!(out.len(), input.len());
    }

    /// CEP test vectors 2 and 5: the placeholder occurs only inside the shebang line, so a
    /// conformant producer records `offsets: []`. With a target prefix well over the 127-byte
    /// Linux limit the patched shebang collapses to the `#!/usr/bin/env <program>` form.
    #[test]
    fn test_replace_long_prefix_in_text_file_offsets() {
        let test_data_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
        let test_file = test_data_dir.join("shebang_test.txt");
        let prefix_placeholder = "/this/is/placeholder";
        let mut target_prefix = "/super/long/".to_string();
        for _ in 0..15 {
            target_prefix.push_str("verylongstring/");
        }
        let input = fs::read(test_file).unwrap();

        // The only occurrence is inside the shebang region, so `offsets` is empty.
        // Derive `shebang_length` (first-newline index + 1) from the file rather than
        // hardcoding it, so the test is robust to a CRLF checkout on Windows — where the
        // extra carriage return shifts the newline and thus the region length.
        let offsets: Vec<usize> = Vec::new();
        let shebang_length = input
            .iter()
            .position(|&c| c == b'\n')
            .map_or(input.len(), |i| i + 1);

        let mut output = Cursor::new(Vec::new());
        super::copy_and_replace_textual_placeholder_offsets(
            &input,
            &mut output,
            prefix_placeholder,
            &target_prefix,
            &Platform::Linux64,
            &offsets,
            Some(shebang_length),
        )
        .unwrap();

        let output = output.into_inner();
        let replaced = String::from_utf8_lossy(&output);
        insta::assert_snapshot!(replaced);
    }
}
