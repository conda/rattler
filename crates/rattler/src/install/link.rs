use crate::utils::{parse_sha256_from_hex, Sha256HashingWriter};
use apple_codesign::{SigningSettings, UnifiedSigner};
use rattler_conda_types::package::{FileMode, PathType, PathsEntry};
use rattler_conda_types::Platform;
use std::fs::Permissions;
use std::io::Write;
use std::path::Path;

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
}

/// The successful result of calling [`link_file`].
pub struct LinkedFile {
    /// True if an existing file already existed and linking overwrote the original file.
    pub clobbered: bool,

    /// If linking generated a different file from the file in the package directory (when a prefix,
    /// is replaced for instance) this field contains the new Sha256 hash.
    pub sha256: Option<sha2::digest::Output<sha2::Sha256>>,
}

/// Installs a single file from a `package_dir` to the the `target_dir`. Replaces any
/// `prefix_placeholder` in the file with the `prefix`.
///
/// `relative_path` is the path of the file in the `package_dir` (and the `target_dir`).
///
/// Note that usually the `target_prefix` is equal to `target_dir` but it might differ. See
/// [`super::InstallOptions::target_prefix`] for more information.
pub fn link_file(
    path_json_entry: &PathsEntry,
    package_dir: &Path,
    target_dir: &Path,
    target_prefix: &str,
    allow_symbolic_links: bool,
    allow_hard_links: bool,
    target_platform: Platform,
) -> Result<LinkedFile, LinkFileError> {
    let destination_path = target_dir.join(&path_json_entry.relative_path);
    let source_path = package_dir.join(&path_json_entry.relative_path);

    // Ensure that all directories up to the path exist.
    if let Some(parent) = destination_path.parent() {
        std::fs::create_dir_all(parent).map_err(LinkFileError::FailedToCreateParentDirectory)?;
    }

    // If the file already exists it most likely means that the file is clobbered. This means that
    // different packages are writing to the same file. This function simply reports back to the
    // caller that this is the case but there is no special handling here.
    let clobbered = destination_path.is_file();

    let sha256 = if let Some(prefix_placeholder) = path_json_entry.prefix_placeholder.as_deref() {
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
        let mut destination_writer = Sha256HashingWriter::new(destination);

        // Replace the prefix placeholder in the file with the new placeholder
        match path_json_entry.file_mode {
            FileMode::Text => {
                copy_and_replace_textual_placeholder(
                    source.as_ref(),
                    &mut destination_writer,
                    prefix_placeholder,
                    target_prefix,
                )?;
            }
            FileMode::Binary => {
                copy_and_replace_cstring_placeholder(
                    source.as_ref(),
                    &mut destination_writer,
                    prefix_placeholder,
                    target_prefix,
                )?;
            }
        }

        let (_, current_hash) = destination_writer.finalize();

        // In case of binary files we have to take care of reconstructing permissions and resigning
        // executables.
        if path_json_entry.file_mode == FileMode::Binary {
            // Copy over filesystem permissions for binary files
            let metadata = std::fs::symlink_metadata(&source_path)
                .map_err(LinkFileError::FailedToReadSourceFileMetadata)?;
            std::fs::set_permissions(&destination_path, metadata.permissions())
                .map_err(LinkFileError::FailedToUpdateDestinationFilePermissions)?;

            // (re)sign the binary if the file is executable
            if has_executable_permissions(&metadata.permissions())
                && target_platform == Platform::OsxArm64
            {
                // Did the binary actually change?
                let original_hash = path_json_entry
                    .sha256
                    .as_deref()
                    .and_then(parse_sha256_from_hex);
                let content_changed = original_hash != Some(current_hash);

                // If the binary changed it requires resigning.
                if content_changed {
                    let signer = UnifiedSigner::new(SigningSettings::default());
                    signer.sign_path_in_place(destination_path)?
                }
            }
        }

        Some(current_hash)
    } else if path_json_entry.path_type == PathType::HardLink && allow_hard_links {
        std::fs::hard_link(&source_path, &destination_path)?;
        None
    } else if path_json_entry.path_type == PathType::SoftLink && allow_symbolic_links {
        let linked_path = source_path
            .read_link()
            .map_err(LinkFileError::FailedToOpenSourceFile)?;
        symlink(&linked_path, &destination_path)?;
        None
    } else {
        std::fs::copy(&source_path, &destination_path)?;
        None
    };

    Ok(LinkedFile { clobbered, sha256 })
}

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text.
///
/// This is a text based version where the complete string is replaced. This works fine for text
/// files but will not work correctly for binary files where the length of the string is often
/// important. See [`copy_and_replace_cstring_placeholder`] when you are dealing with binary
/// content.
fn copy_and_replace_textual_placeholder(
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
fn copy_and_replace_cstring_placeholder(
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
        b"12345Hello, fabulous world!\06789",
        "fabulous",
        "cruel",
        b"12345Hello, cruel world!\0\0\0\06789"
    )]
    #[case(b"short\0", "short", "verylong", b"veryl\0")]
    #[case(b"short1234\0", "short", "verylong", b"verylong1\0")]
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
