use crate::install::python::PythonInfo;
use apple_codesign::{SigningSettings, UnifiedSigner};
use rattler_conda_types::package::{FileMode, PathType, PathsEntry, PrefixPlaceholder};
use rattler_conda_types::{NoArchType, Platform};
use rattler_digest::Sha256;
use rattler_digest::{parse_digest_from_hex, HashingWriter};
use std::borrow::Cow;
use std::fs::Permissions;
use std::io::{ErrorKind, Seek, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LinkFileError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error("failed to create parent directory")]
    FailedToCreateParentDirectory(#[source] std::io::Error),

    #[error("could not open source file")]
    FailedToOpenSourceFile(#[source] std::io::Error),

    #[error("could not source file metadata")]
    FailedToReadSourceFileMetadata(#[source] std::io::Error),

    #[error("could not open destination file for writing")]
    FailedToOpenDestinationFile(#[source] std::io::Error),

    #[error("could not update destination file permissions")]
    FailedToUpdateDestinationFilePermissions(#[source] std::io::Error),

    #[error("failed to sign Apple binary")]
    FailedToSignAppleBinary(#[from] apple_codesign::AppleCodesignError),

    #[error("cannot install noarch python files because there is no python version specified ")]
    MissingPythonInfo,
}

/// The successful result of calling [`link_file`].
pub struct LinkedFile {
    /// True if an existing file already existed and linking overwrote the original file.
    pub clobbered: bool,

    /// The SHA256 hash of the resulting file.
    pub sha256: rattler_digest::Sha256Array,

    /// The size of the final file in bytes.
    pub file_size: u64,

    /// The relative path of the file in the destination directory. This might be different from the
    /// relative path in the source directory for python noarch packages.
    pub relative_path: PathBuf,
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
    noarch_type: NoArchType,
    path_json_entry: &PathsEntry,
    package_dir: &Path,
    target_dir: &Path,
    target_prefix: &str,
    allow_symbolic_links: bool,
    allow_hard_links: bool,
    target_platform: Platform,
    target_python: Option<&PythonInfo>,
) -> Result<LinkedFile, LinkFileError> {
    let source_path = package_dir.join(&path_json_entry.relative_path);

    // Determine the destination path
    let destination_relative_path = if noarch_type.is_python() {
        match target_python {
            Some(python_info) => {
                python_info.get_python_noarch_target_path(&path_json_entry.relative_path)
            }
            None => return Err(LinkFileError::MissingPythonInfo),
        }
    } else {
        path_json_entry.relative_path.as_path().into()
    };
    let destination_path = target_dir.join(&destination_relative_path);

    // Ensure that all directories up to the path exist.
    if let Some(parent) = destination_path.parent() {
        std::fs::create_dir_all(parent).map_err(LinkFileError::FailedToCreateParentDirectory)?;
    }

    // If the file already exists it most likely means that the file is clobbered. This means that
    // different packages are writing to the same file. This function simply reports back to the
    // caller that this is the case but there is no special handling here.
    let clobbered = destination_path.is_file();

    // Temporary variables to store intermediate computations in. If we already computed the file
    // size or the sha hash we dont have to recompute them at the end of the function.
    let mut sha256 = None;
    let mut file_size = path_json_entry.size_in_bytes;

    if let Some(PrefixPlaceholder {
        file_mode,
        placeholder,
    }) = path_json_entry.prefix_placeholder.as_ref()
    {
        // Memory map the source file. This provides us with easy access to a continuous stream of
        // bytes which makes it easier to search for the placeholder prefix.
        let source = {
            let file =
                std::fs::File::open(&source_path).map_err(LinkFileError::FailedToOpenSourceFile)?;
            unsafe { memmap2::Mmap::map(&file).map_err(LinkFileError::FailedToOpenSourceFile)? }
        };

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
        copy_and_replace_placholders(
            source.as_ref(),
            &mut destination_writer,
            placeholder,
            &target_prefix,
            *file_mode,
        )?;

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
            let original_hash = path_json_entry
                .sha256
                .as_deref()
                .and_then(parse_digest_from_hex::<rattler_digest::Sha256>);
            let content_changed = original_hash != Some(current_hash);

            // If the binary changed it requires resigning.
            if content_changed {
                let signer = UnifiedSigner::new(SigningSettings::default());
                signer.sign_path_in_place(&destination_path)?;

                // The file on disk changed from the original file so the hash and file size
                // also became invalid.
                sha256 = None;
                file_size = None;
            }
        }
    } else if path_json_entry.path_type == PathType::HardLink && allow_hard_links {
        loop {
            match std::fs::hard_link(&source_path, &destination_path) {
                Ok(_) => break,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    std::fs::remove_file(&destination_path)?;
                }
                Err(e) => return Err(e.into()),
            }
        }
    } else if path_json_entry.path_type == PathType::SoftLink && allow_symbolic_links {
        let linked_path = source_path
            .read_link()
            .map_err(LinkFileError::FailedToOpenSourceFile)?;

        loop {
            match symlink(&linked_path, &destination_path) {
                Ok(_) => break,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    std::fs::remove_file(&destination_path)?;
                }
                Err(e) => return Err(e.into()),
            }
        }
    } else {
        loop {
            match std::fs::copy(&source_path, &destination_path) {
                Ok(_) => break,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    std::fs::remove_file(&destination_path)?;
                }
                Err(e) => return Err(e.into()),
            }
        }
    };

    // Compute the final SHA256 if we didnt already or if its not stored in the paths.json entry.
    let sha256 = if let Some(sha256) = sha256 {
        sha256
    } else if let Some(sha256) = path_json_entry
        .sha256
        .as_deref()
        .and_then(rattler_digest::parse_digest_from_hex::<Sha256>)
    {
        sha256
    } else {
        rattler_digest::compute_file_digest::<Sha256>(&destination_path)
            .map_err(LinkFileError::FailedToOpenDestinationFile)?
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

    Ok(LinkedFile {
        clobbered,
        sha256,
        file_size,
        relative_path: destination_relative_path.into_owned(),
    })
}

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text.
///
/// This switches to more specialized functions that handle the replacement of either
/// textual and binary placeholders, the [`FileMode`] enum switches between the two functions.
/// See both [`copy_and_replace_cstring_placeholder`] and [`copy_and_replace_textual_placeholder`]
pub fn copy_and_replace_placholders(
    source_bytes: &[u8],
    destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    file_mode: FileMode,
) -> Result<(), std::io::Error> {
    match file_mode {
        FileMode::Text => {
            copy_and_replace_textual_placeholder(
                source_bytes,
                destination,
                prefix_placeholder,
                target_prefix,
            )?;
        }
        FileMode::Binary => {
            copy_and_replace_cstring_placeholder(
                source_bytes,
                destination,
                prefix_placeholder,
                target_prefix,
            )?;
        }
    }
    Ok(())
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
) -> Result<(), std::io::Error> {
    // Get the prefixes as bytes
    let old_prefix = prefix_placeholder.as_bytes();
    let new_prefix = target_prefix.as_bytes();

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

    // Compute the padding required when replacing the old prefix with the new one. If the old
    // prefix is longer than the new one we need to add padding to ensure that the entire part
    // will hold the same number of bytes. We do this by adding '\0's (e.g. nul terminators). This
    // ensures that the text will remain a valid nul-terminated string.
    let padding = vec![b'\0'; old_prefix.len().saturating_sub(new_prefix.len())];

    loop {
        if let Some(index) = memchr::memmem::find(source_bytes, old_prefix) {
            // Find the end of the c-style string. The nul terminator basically.
            let mut end = index + old_prefix.len();
            while end < source_bytes.len() && source_bytes[end] != b'\0' {
                end += 1;
            }

            // Determine the total length of the c-string.
            let len = end - index;

            // Get the suffix part (this is the text after the prefix by up until the nul
            // terminator). E.g. in `old-prefix/some/path\0` the suffix would be `/some/path`.
            let suffix = &source_bytes[index + old_prefix.len()..end];

            // Write all bytes up to the old prefix, then the new prefix followed by suffix and
            // padding.
            destination.write_all(&source_bytes[..index])?;
            destination.write_all(&new_prefix[..len.min(new_prefix.len())])?;
            destination
                .write_all(&suffix[..len.saturating_sub(new_prefix.len()).min(suffix.len())])?;
            destination.write_all(&padding)?;

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
    use rstest::rstest;
    use std::io::Cursor;

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
}
