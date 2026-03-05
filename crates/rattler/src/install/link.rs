//! This module contains the logic to link a give file from the package cache into the target directory.
//! See [`link_file`] for more information.
use fs_err as fs;
use memmap2::Mmap;
use once_cell::sync::Lazy;
use rattler_conda_types::package::{FileMode, PathType, PathsEntry, PrefixPlaceholder};
use rattler_conda_types::Platform;
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

use super::apple_codesign::{codesign, AppleCodeSignBehavior};
use super::Prefix;

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
    allow_external_symlinks: bool,
) -> Result<LinkedFile, LinkFileError> {
    let source_path = package_dir.join(&path_json_entry.relative_path);

    let destination_path = target_dir.path().join(&destination_relative_path);

    // Temporary variables to store intermediate computations in. If we already computed the file
    // size or the sha hash we don't have to recompute them at the end of the function.
    let mut sha256 = None;
    let mut file_size = path_json_entry.size_in_bytes;

    let link_method = if let Some(PrefixPlaceholder {
        file_mode,
        placeholder,
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

        // Replace the prefix placeholder in the file with the new placeholder
        copy_and_replace_placeholders(
            source.as_ref(),
            &mut destination_writer,
            placeholder,
            &target_prefix,
            &target_platform,
            *file_mode,
        )
        .map_err(|err| LinkFileError::IoError(String::from("replacing placeholders"), err))?;

        let (mut file, current_hash) = destination_writer.finalize();

        // We computed the hash of the file while writing and from the file we can also infer the
        // size of it.
        sha256 = Some(current_hash);
        file_size = file.stream_position().ok();

        // We no longer need the file.
        drop(file);

        let metadata = fs::symlink_metadata(&source_path)
            .map_err(LinkFileError::FailedToReadSourceFileMetadata)?;
        // (re)sign the binary if the file is executable or is a Mach-O binary (e.g., dylib)
        // This is required for all macOS platforms because prefix replacement modifies the binary
        // content, which invalidates existing signatures. We need to preserve entitlements.
        if (has_executable_permissions(&metadata.permissions())
            || file_type == Some(FileType::MachO))
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
    } else if path_json_entry.path_type == PathType::SoftLink && allow_symbolic_links {
        symlink_to_destination(
            &source_path,
            &destination_path,
            target_dir.path(),
            allow_external_symlinks,
        )?
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

    Ok(LinkedFile {
        clobbered: false,
        sha256,
        file_size,
        relative_path: destination_relative_path,
        method: link_method,
        prefix_placeholder,
    })
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
    allow_external_symlinks: bool,
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
        if allow_external_symlinks {
            tracing::warn!(
                "symlink {} points outside the target prefix: {}",
                destination_path.display(),
                linked_path.display()
            );
        } else {
            return Err(LinkFileError::SymlinkTargetEscapesPrefix);
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
                return copy_to_destination(source_path, destination_path);
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
        // extract first line
        let (first, rest) =
            source_bytes.split_at(source_bytes.iter().position(|&c| c == b'\n').unwrap_or(0));
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
    use super::PYTHON_REGEX;
    use fs_err as fs;
    use rattler_conda_types::Platform;
    use rstest::rstest;
    use std::io::Cursor;

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
            false,
        )
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
            false,
        )
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
        use super::{symlink_to_destination, LinkFileError};

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
            false,
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
            false,
        );
        assert!(result.is_ok());
    }
}
