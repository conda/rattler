//! This module contains the logic to link a give file from the package cache into the target directory.
//! See [`link_file`] for more information.
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
use std::io::{ErrorKind, Read, Seek, Write};
use std::path::{Path, PathBuf};

use super::apple_codesign::{codesign, AppleCodeSignBehavior};

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

    /// The binary (dylib or executable) could not be signed (codesign -f -s -) on
    /// macOS ARM64 (Apple Silicon).
    #[error("failed to sign Apple binary")]
    FailedToSignAppleBinary,

    /// No Python version was specified when installing a noarch package.
    #[error("cannot install noarch python files because there is no python version specified ")]
    MissingPythonInfo,

    /// The hash of the file could not be computed.
    #[error("failed to compute the sha256 hash of the file")]
    FailedToComputeSha(#[source] std::io::Error),
}

/// The successful result of calling [`link_file`].
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
#[allow(clippy::too_many_arguments)] // TODO: Fix this properly
pub fn link_file(
    path_json_entry: &PathsEntry,
    destination_relative_path: PathBuf,
    package_dir: &Path,
    target_dir: &Path,
    target_prefix: &str,
    allow_symbolic_links: bool,
    allow_hard_links: bool,
    allow_ref_links: bool,
    target_platform: Platform,
    apple_codesign_behavior: AppleCodeSignBehavior,
) -> Result<LinkedFile, LinkFileError> {
    let source_path = package_dir.join(&path_json_entry.relative_path);

    let destination_path = target_dir.join(&destination_relative_path);

    // Temporary variables to store intermediate computations in. If we already computed the file
    // size or the sha hash we dont have to recompute them at the end of the function.
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

        // Open the destination file
        let destination = std::fs::File::create(&destination_path)
            .map_err(LinkFileError::FailedToOpenDestinationFile)?;
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

        // Copy over filesystem permissions. We do this to ensure that the destination file has the
        // same permissions as the source file.
        let metadata = std::fs::symlink_metadata(&source_path)
            .map_err(LinkFileError::FailedToReadSourceFileMetadata)?;
        std::fs::set_permissions(&destination_path, metadata.permissions())
            .map_err(LinkFileError::FailedToUpdateDestinationFilePermissions)?;

        // (re)sign the binary if the file is executable
        if has_executable_permissions(&metadata.permissions())
            && target_platform == Platform::OsxArm64
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
                    std::fs::symlink_metadata(&destination_path)
                        .map_err(LinkFileError::FailedToOpenDestinationFile)?
                        .len(),
                );
            }
        }
        LinkMethod::Patched(*file_mode)
    } else if path_json_entry.path_type == PathType::HardLink && allow_ref_links {
        reflink_to_destination(&source_path, &destination_path, allow_hard_links)?
    } else if path_json_entry.path_type == PathType::HardLink && allow_hard_links {
        hardlink_to_destination(&source_path, &destination_path)?
    } else if path_json_entry.path_type == PathType::SoftLink && allow_symbolic_links {
        symlink_to_destination(&source_path, &destination_path)?
    } else {
        copy_to_destination(&source_path, &destination_path)?
    };

    // Compute the final SHA256 if we didnt already or if its not stored in the paths.json entry.
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

    // Compute the final file size if we didnt already.
    let file_size = if let Some(file_size) = file_size {
        file_size
    } else if let Some(size_in_bytes) = path_json_entry.size_in_bytes {
        size_in_bytes
    } else {
        let metadata = std::fs::symlink_metadata(&destination_path)
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
    let mut file =
        std::fs::File::open(source_path).map_err(LinkFileError::FailedToOpenSourceFile)?;

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
                // Copy over filesystem permissions. We do this to ensure that the destination file has the
                // same permissions as the source file.
                let metadata = std::fs::metadata(source_path)
                    .map_err(LinkFileError::FailedToReadSourceFileMetadata)?;
                std::fs::set_permissions(destination_path, metadata.permissions())
                    .map_err(LinkFileError::FailedToUpdateDestinationFilePermissions)?;
                return Ok(LinkMethod::Reflink);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                std::fs::remove_file(destination_path).map_err(|err| {
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
        match std::fs::hard_link(source_path, destination_path) {
            Ok(_) => return Ok(LinkMethod::Hardlink),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                std::fs::remove_file(destination_path).map_err(|err| {
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
) -> Result<LinkMethod, LinkFileError> {
    let linked_path = source_path
        .read_link()
        .map_err(LinkFileError::FailedToReadSymlink)?;
    loop {
        match symlink(&linked_path, destination_path) {
            Ok(_) => return Ok(LinkMethod::Softlink),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                std::fs::remove_file(destination_path).map_err(|err| {
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
        match std::fs::copy(source_path, destination_path) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // If the file already exists, remove it and try again.
                std::fs::remove_file(destination_path).map_err(|err| {
                    LinkFileError::IoError(String::from("removing clobbered file"), err)
                })?;
            }
            Ok(_) => return Ok(LinkMethod::Copy),
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

    loop {
        if let Some(index) = memchr::memmem::find(source_bytes, old_prefix) {
            // Write all bytes up to the old prefix, followed by the new prefix.
            destination.write_all(&source_bytes[..index])?;
            destination.write_all(new_prefix)?;

            // Skip past the old prefix in the source bytes
            source_bytes = &source_bytes[index + old_prefix.len()..];
        } else {
            // The old prefix was not found in the (remaining) source bytes.
            // Write the rest of the bytes
            destination.write_all(source_bytes)?;
            return Ok(());
        }
    }
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

    loop {
        if let Some(index) = memchr::memmem::find(source_bytes, old_prefix) {
            // write all bytes up to the old prefix, followed by the new prefix.
            destination.write_all(&source_bytes[..index])?;

            // Find the end of the c-style string. The nul terminator basically.
            let mut end = index + old_prefix.len();
            while end < source_bytes.len() && source_bytes[end] != b'\0' {
                end += 1;
            }

            let mut out = Vec::new();
            let mut old_bytes = &source_bytes[index..end];
            let old_len = old_bytes.len();

            // replace all occurrences of the old prefix with the new prefix
            while let Some(index) = memchr::memmem::find(old_bytes, old_prefix) {
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
            // will hold the same number of bytes. We do this by adding '\0's (e.g. nul terminators). This
            // ensures that the text will remain a valid nul-terminated string.
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
    return std::os::windows::fs::symlink_file(source_path, destination_path);
    #[cfg(unix)]
    return std::os::unix::fs::symlink(source_path, destination_path);
}

#[allow(unused_variables)]
fn has_executable_permissions(permissions: &Permissions) -> bool {
    #[cfg(windows)]
    return false;
    #[cfg(unix)]
    return std::os::unix::fs::PermissionsExt::mode(permissions) & 0o111 != 0;
}

#[cfg(test)]
mod test {
    use rattler_conda_types::Platform;
    use rstest::rstest;
    use std::io::Cursor;

    use super::PYTHON_REGEX;

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
    #[case(b"short\x00", "short", "verylong", b"veryl\x00")]
    #[case(b"short1234\x00", "short", "verylong", b"verylong1\x00")]
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
        let input = std::fs::read(test_file).unwrap();
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
}
