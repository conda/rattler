use super::InstallError;
use rattler_conda_types::package::{FileMode, PathType};
use sha2::{Digest, Sha256};
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

    #[error("could not open destination file for writing")]
    FailedToOpenDestinationFile(#[source] std::io::Error),
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
    relative_path: &Path,
    package_dir: &Path,
    target_dir: &Path,
    target_prefix: &str,
    prefix_placeholder: Option<&str>,
    path_type: PathType,
    file_mode: FileMode,
    always_copy: bool,
) -> Result<(), LinkFileError> {
    let destination_path = target_dir.join(relative_path);
    let source_path = package_dir.join(relative_path);

    // Ensure that all directories up to the path exist.
    if let Some(parent) = destination_path.parent() {
        std::fs::create_dir_all(parent).map_err(LinkFileError::FailedToCreateParentDirectory)?;
    }

    // If the file already exists it most likely means that the file is clobbered. This means that
    // different packages are writing to the same file. This function simply reports back to the
    // caller that this is the case but there is no special handling here.
    let clobber = destination_path.is_file();

    // let sha256 = if let Some(prefix_placeholder) = prefix_placeholder {
    //     match file_mode {
    //         FileMode::Text => {
    //             copy_and_replace_text(&source_path, &destination_path, prefix_placeholder, target_prefix)
    //         },
    //         FileMode::Binary => {
    //
    //         },
    //     }
    // }

    Ok(())
}

/// Given a file in a package archive, copy it over to the destination and immediately replace the
/// placeholder prefix in the file with the the new prefix.
///
/// This is a text based version where the complete string is replaced. This works fine for text
/// files but will not work correctly for binary files where the length of the string is often
/// important. See [`link_binary_file`] when you are dealing with binary files.
///
/// TODO: This function should also update shebangs.
fn link_text_file(
    source_path: &Path,
    destination_path: &Path,
    prefix_placeholder: &str,
    target_prefix: &str,
) -> Result<sha2::digest::Output<sha2::Sha256>, LinkFileError> {
    // Memory map the source file. This provides us with easy access to a continuous stream of
    // bytes which makes it easier to search for the placeholder prefix.
    let source = {
        let file =
            std::fs::File::open(source_path).map_err(LinkFileError::FailedToOpenSourceFile)?;
        unsafe { memmap2::Mmap::map(&file).map_err(LinkFileError::FailedToOpenSourceFile)? }
    };

    // Open the output file for writing
    let mut destination = std::fs::File::create(destination_path)
        .map_err(LinkFileError::FailedToOpenDestinationFile)?;

    // Get the prefixes as bytes
    let old_prefix = prefix_placeholder.as_bytes();
    let new_prefix = target_prefix.as_bytes();

    let mut hasher = Sha256::new();
    let mut source_bytes = source.as_ref();
    loop {
        if let Some(index) = memchr::memmem::find(source_bytes, old_prefix) {
            // Write all bytes up to the old prefix, followed by the new prefix.
            destination.write_all(&source_bytes[..index])?;
            destination.write_all(new_prefix)?;

            // Update digest with the same bytes
            hasher.update(&source_bytes[..index]);
            hasher.update(new_prefix);

            // Skip past the old prefix in the source bytes
            source_bytes = &source_bytes[index + old_prefix.len()..];
        } else {
            // The old prefix was not found in the (remaining) source bytes.
            // Write the rest of the bytes to disk
            destination.write_all(&source_bytes)?;

            // Update the digest with the same bytes
            hasher.update(&source_bytes);

            // Return the final hash
            return Ok(hasher.finalize());
        }
    }
}

#[cfg(test)]
mod test {
    use tempfile::tempdir;

    #[test]
    pub fn test_link_text_file() {
        // Write a text file to disk
        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("bla.txt");
        std::fs::write(&source_path, "Hello, cruel world!").unwrap();

        // Link the text file. This will replace the bytes "cruel" with "fabulous".
        let destination_path = source_dir.path().join("bla2.txt");
        let digest =
            super::link_text_file(&source_path, &destination_path, "cruel", "fabulous").unwrap();

        // Check the contents of the linked file
        let updated_content = std::fs::read_to_string(&destination_path).unwrap();
        assert_eq!(&updated_content, "Hello, fabulous world!");

        // Make sure that the digest matches
        assert_eq!(
            crate::utils::compute_file_sha256(&destination_path).unwrap(),
            digest
        );
    }
}
