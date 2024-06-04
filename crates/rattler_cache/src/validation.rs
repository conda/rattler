//! Functionality to validate the contents of a Conda package.
//!
//! Almost all Conda packages contain a file `info/paths.json` that describes all the files the
//! package contains. The [`validate_package_directory`] function validates that a directory
//! containing an extracted Conda package archive actually contains the files as described by the
//! `paths.json` file.
//!
//! Very old Conda packages do not contain a `paths.json` file. These packages contain a
//! (deprecated) `files` file as well as optionally a `has_prefix` and some other files. If the
//! `paths.json` file is missing these deprecated files are used instead to reconstruct a
//! [`PathsJson`] object. See [`PathsJson::from_deprecated_package_directory`] for more information.

use digest::Digest;
use rattler_conda_types::package::{IndexJson, PackageFile, PathType, PathsEntry, PathsJson};
use rattler_digest::Sha256;
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

/// An error that is returned by [`validate_package_directory`] if the contents of the directory seems to be
/// corrupted.
#[derive(Debug, thiserror::Error)]
pub enum PackageValidationError {
    /// Neither a `paths.json` file nor a deprecated `files` file was found.
    #[error("neither a 'paths.json' or a deprecated 'files' file was found")]
    MetadataMissing,

    /// An error occurred while reading the `paths.json` file.
    #[error("failed to read 'paths.json' file")]
    ReadPathsJsonError(#[source] std::io::Error),

    /// An error occurred while reading the deprecated `files` file.
    #[error("failed to read validation data from deprecated files")]
    ReadDeprecatedPathsJsonError(#[source] std::io::Error),

    /// The path seems to be corrupted.
    #[error("the path '{0}' seems to be corrupted")]
    CorruptedEntry(PathBuf, #[source] PackageEntryValidationError),

    /// An error occurred while reading the `index.json` file.
    #[error("failed to read 'index.json'")]
    ReadIndexJsonError(#[source] std::io::Error),
}

/// An error that indicates that a specific file in a package archive directory seems to be corrupted.
#[derive(Debug, thiserror::Error)]
pub enum PackageEntryValidationError {
    /// An error occurred while reading the metadata of the file.
    #[error("failed to retrieve file metadata'")]
    GetMetadataFailed(#[source] std::io::Error),

    /// The file does not exist.
    #[error("the file does not exist")]
    NotFound,

    /// The file is not a symbolic link.
    #[error("expected a symbolic link")]
    ExpectedSymlink,

    /// The file is not a directory.
    #[error("expected a directory")]
    ExpectedDirectory,

    /// The size of the file does not match the expected size.
    #[error("incorrect size, expected {0} but file on disk is {1}")]
    IncorrectSize(u64, u64),

    /// An IO error occurred while reading the file.
    #[error("an io error occurred")]
    IoError(#[from] std::io::Error),

    /// The SHA256 hash of the file does not match the expected hash.
    #[error("sha256 hash mismatch, expected '{0}' but file on disk is '{1}'")]
    HashMismatch(String, String),
}

/// Determine whether the files in the specified directory match what is expected according to the
/// `info/paths.json` file in the same directory.
///
/// If the `info/paths.json` file could not be found this function tries to reconstruct the
/// information from older deprecated methods. See [`PathsJson::from_deprecated_package_directory`].
///
/// If validation succeeds the parsed [`PathsJson`] object is returned which contains information
/// about the files in the archive.
pub fn validate_package_directory(
    package_dir: &Path,
) -> Result<(IndexJson, PathsJson), PackageValidationError> {
    // Validate that there is a valid IndexJson
    let index_json = IndexJson::from_package_directory(package_dir)
        .map_err(PackageValidationError::ReadIndexJsonError)?;

    // Read the 'paths.json' file which describes all files that should be present. If the file
    // could not be found try reconstructing the paths information from deprecated files in the
    // package directory.
    let paths = match PathsJson::from_package_directory(package_dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            match PathsJson::from_deprecated_package_directory(package_dir) {
                Ok(paths) => paths,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    return Err(PackageValidationError::MetadataMissing)
                }
                Err(e) => return Err(PackageValidationError::ReadDeprecatedPathsJsonError(e)),
            }
        }
        Err(e) => return Err(PackageValidationError::ReadPathsJsonError(e)),
        Ok(paths) => paths,
    };

    // Validate all the entries
    validate_package_directory_from_paths(package_dir, &paths)
        .map_err(|(path, err)| PackageValidationError::CorruptedEntry(path, err))?;

    Ok((index_json, paths))
}

/// Determine whether the files in the specified directory match wat is expected according to the
/// passed in [`PathsJson`].
pub fn validate_package_directory_from_paths(
    package_dir: &Path,
    paths: &PathsJson,
) -> Result<(), (PathBuf, PackageEntryValidationError)> {
    // Check every entry in the PathsJson object
    for entry in paths.paths.iter() {
        validate_package_entry(package_dir, entry).map_err(|e| (entry.relative_path.clone(), e))?;
    }

    Ok(())
}

/// Determine whether the information in the [`PathsEntry`] matches the file in the package directory.
fn validate_package_entry(
    package_dir: &Path,
    entry: &PathsEntry,
) -> Result<(), PackageEntryValidationError> {
    let path = package_dir.join(&entry.relative_path);

    // Validate based on the type of path
    match entry.path_type {
        PathType::HardLink => validate_package_hard_link_entry(path, entry),
        PathType::SoftLink => validate_package_soft_link_entry(path, entry),
        PathType::Directory => validate_package_directory_entry(path, entry),
    }
}

/// Determine whether the information in the [`PathsEntry`] matches the file at the specified path.
fn validate_package_hard_link_entry(
    path: PathBuf,
    entry: &PathsEntry,
) -> Result<(), PackageEntryValidationError> {
    debug_assert!(entry.path_type == PathType::HardLink);

    // Short-circuit if we have no validation reference
    if entry.sha256.is_none() && entry.size_in_bytes.is_none() {
        if !path.is_file() {
            return Err(PackageEntryValidationError::NotFound);
        }
        return Ok(());
    }

    // Open the file for reading
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(PackageEntryValidationError::NotFound);
        }
        Err(e) => return Err(PackageEntryValidationError::IoError(e)),
    };

    // Validate the size of the file
    if let Some(size_in_bytes) = entry.size_in_bytes {
        let actual_file_len = file
            .metadata()
            .map_err(PackageEntryValidationError::IoError)?
            .len();
        if size_in_bytes != actual_file_len {
            return Err(PackageEntryValidationError::IncorrectSize(
                size_in_bytes,
                actual_file_len,
            ));
        }
    }

    // Check the SHA256 hash of the file
    if let Some(expected_hash) = &entry.sha256 {
        // Determine the hash of the file on disk
        let mut hasher = Sha256::default();
        std::io::copy(&mut file, &mut hasher)?;
        let hash = hasher.finalize();

        // Compare the two hashes
        if expected_hash != &hash {
            return Err(PackageEntryValidationError::HashMismatch(
                format!("{expected_hash:x}"),
                format!("{hash:x}"),
            ));
        }
    }

    Ok(())
}

/// Determine whether the information in the [`PathsEntry`] matches the symbolic link at the specified
/// path.
fn validate_package_soft_link_entry(
    path: PathBuf,
    entry: &PathsEntry,
) -> Result<(), PackageEntryValidationError> {
    debug_assert!(entry.path_type == PathType::SoftLink);

    if !path.is_symlink() {
        return Err(PackageEntryValidationError::ExpectedSymlink);
    }

    // TODO: Validate symlink content. Dont validate the SHA256 hash of the file because since a
    // symlink will most likely point to another file added as a hardlink by the package this is
    // double work. Instead check that the symlink is correct e.g. `../a` points to the same file as
    // `b/../../a` but they are different.

    Ok(())
}

/// Determine whether the information in the [`PathsEntry`] matches the directory at the specified path.
fn validate_package_directory_entry(
    path: PathBuf,
    entry: &PathsEntry,
) -> Result<(), PackageEntryValidationError> {
    debug_assert!(entry.path_type == PathType::Directory);

    if path.is_dir() {
        Ok(())
    } else {
        Err(PackageEntryValidationError::ExpectedDirectory)
    }
}

#[cfg(test)]
mod test {
    use super::{
        validate_package_directory, validate_package_directory_from_paths,
        PackageEntryValidationError, PackageValidationError,
    };
    use assert_matches::assert_matches;
    use rattler_conda_types::package::{PackageFile, PathType, PathsJson};
    use rstest::rstest;
    use std::io::Write;
    use url::Url;

    #[rstest]
    #[case::conda(
        "https://conda.anaconda.org/conda-forge/win-64/conda-22.9.0-py38haa244fe_2.tar.bz2",
        "3c2c2e8e81bde5fb1ac4b014f51a62411feff004580c708c97a0ec2b7058cdc4"
    )]
    #[case::mamba(
        "https://conda.anaconda.org/conda-forge/win-64/mamba-1.0.0-py38hecfeebb_2.tar.bz2",
        "f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97"
    )]
    #[case::conda(
        "https://conda.anaconda.org/conda-forge/win-64/conda-22.11.1-py38haa244fe_1.conda",
        "a8a44c5ff2b2f423546d49721ba2e3e632233c74a813c944adf8e5742834930e"
    )]
    #[case::mamba(
        "https://conda.anaconda.org/conda-forge/win-64/mamba-1.1.0-py39hb3d9227_2.conda",
        "c172acdf9cb7655dd224879b30361a657b09bb084b65f151e36a2b51e51a080a"
    )]
    fn test_validate_package_files(#[case] url: Url, #[case] sha256: &str) {
        // Create a temporary directory and extract the given package.
        let temp_dir = tempfile::tempdir().unwrap();
        let package_path = tools::download_and_cache_file(url, sha256).unwrap();

        rattler_package_streaming::fs::extract(&package_path, temp_dir.path()).unwrap();

        // Validate that the extracted package is correct. Since it's just been extracted this should
        // work.
        let result = validate_package_directory(temp_dir.path());
        if let Err(e) = result {
            panic!("{e}");
        }

        // Read the paths.json file and select the first file in the archive.
        let paths = PathsJson::from_package_directory(temp_dir.path())
            .or_else(|_| PathsJson::from_deprecated_package_directory(temp_dir.path()))
            .unwrap();
        let entry = paths
            .paths
            .iter()
            .find(|e| e.path_type == PathType::HardLink)
            .expect("package does not contain a file");

        // Change the file by writing a single character to the start of the file
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(temp_dir.path().join(&entry.relative_path))
            .unwrap();
        file.write_all(&[255]).unwrap();
        drop(file);

        // Revalidate the package, given that we changed a file it should now fail with mismatched hashes.
        assert_matches!(
            validate_package_directory_from_paths(temp_dir.path(), &paths),
            Err((
                path,
                PackageEntryValidationError::HashMismatch(_, _)
            )) if path == entry.relative_path
        );
    }

    #[rstest]
    #[cfg(unix)]
    #[case::mamba(
        "https://conda.anaconda.org/conda-forge/linux-ppc64le/python-3.10.6-h2c4edbf_0_cpython.tar.bz2",
        "978c122f6529cb617b90e6e692308a5945bf9c3ba0c27acbe4bea4c8b02cdad0"
    )]
    // Very old file with deprecated paths.json
    #[case::mamba(
        "https://conda.anaconda.org/conda-forge/linux-64/zlib-1.2.8-3.tar.bz2",
        "85fcb6906b8686fe6341db89b4e6fc2631ad69ee6eab2f4823bfd64ae0b20ac8"
    )]
    fn test_validate_package_files_symlink(#[case] url: Url, #[case] sha256: &str) {
        // Create a temporary directory and extract the given package.
        let temp_dir = tempfile::tempdir().unwrap();
        let package_path = tools::download_and_cache_file(url, sha256).unwrap();

        rattler_package_streaming::fs::extract(&package_path, temp_dir.path()).unwrap();

        // Validate that the extracted package is correct. Since it's just been extracted this should
        // work.
        let result = validate_package_directory(temp_dir.path());
        if let Err(e) = result {
            panic!("{e}");
        }

        // Read the paths.json file and select the first symlink in the archive.
        let paths = PathsJson::from_package_directory(temp_dir.path())
            .or_else(|_| PathsJson::from_deprecated_package_directory(temp_dir.path()))
            .unwrap();
        let entry = paths
            .paths
            .iter()
            .find(|e| e.path_type == PathType::SoftLink)
            .expect("package does not contain a file");

        // Replace the symlink with its content
        let entry_path = temp_dir.path().join(&entry.relative_path);
        let contents = std::fs::read(&entry_path).unwrap();
        std::fs::remove_file(&entry_path).unwrap();
        std::fs::write(entry_path, contents).unwrap();

        // Revalidate the package, given that we replaced the symlink, it should fail.
        assert_matches!(
            validate_package_directory_from_paths(temp_dir.path(), &paths),
            Err((
                path,
                PackageEntryValidationError::ExpectedSymlink
            )) if path == entry.relative_path
        );
    }

    #[test]
    fn test_missing_metadata() {
        let temp_dir = tempfile::tempdir().unwrap();
        assert_matches!(
            validate_package_directory(temp_dir.path()),
            Err(PackageValidationError::ReadIndexJsonError(_))
        );
    }
}
