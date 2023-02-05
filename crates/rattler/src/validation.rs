//! Functionality to validate the contents of a Conda package archive.
//!
//! All Conda packages contain a file `info/paths.json` that describes all the files the package contains.
//! The [`validate_package_files`] function validates that a directory containing an extracted Conda
//! package archive actually contains the files as described by the `paths.json` file.

use rattler_conda_types::package::{PathType, PathsEntry, PathsJson};
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};

/// An error that is returned by [`validate_package_files`] if the contents of the directory seems to be
/// corrupted.
#[derive(Debug, thiserror::Error)]
pub enum PackageValidationError {
    #[error("failed to read 'paths.json' file")]
    ReadPathsJsonError(#[source] std::io::Error),

    #[error("the path '{0}' seems to be corrupted")]
    CorruptedEntry(PathBuf, #[source] PackageEntryValidationError),
}

/// An error that indicates that a specific file in a package archive directory seems to be corrupted.
#[derive(Debug, thiserror::Error)]
pub enum PackageEntryValidationError {
    #[error("failed to retrieve file metadata'")]
    GetMetadataFailed(#[source] std::io::Error),

    #[error("the file does not exist")]
    NotFound,

    #[error("expected a symbolic link")]
    ExpectedSymlink,

    #[error("expected a directory")]
    ExpectedDirectory,

    #[error("incorrect size, expected {0} but file on disk is {1}")]
    IncorrectSize(u64, u64),

    #[error("an io error occurred")]
    IoError(#[from] std::io::Error),

    #[error("sha256 hash mismatch, expected '{0}' but file on disk is '{1}'")]
    HashMismatch(String, String),
}

/// Determine whether the files in the specified directory match what is expected according to the
/// `info/paths.json` file in the same directory.
pub fn validate_package_files(package_dir: &Path) -> Result<(), PackageValidationError> {
    // Read the 'paths.json' file which describes all files that should be present
    let paths = PathsJson::from_path(&package_dir.join("info/paths.json"))
        .map_err(PackageValidationError::ReadPathsJsonError)?;

    // Check every entry
    for entry in paths.paths {
        validate_package_entry(package_dir, &entry)
            .map_err(|e| PackageValidationError::CorruptedEntry(entry.relative_path, e))?;
    }

    Ok(())
}

/// Determine whether the information in the [`PathsEntry`] matches the file in the package directory.
fn validate_package_entry(
    package_dir: &Path,
    entry: &PathsEntry,
) -> Result<(), PackageEntryValidationError> {
    let path = package_dir.join(&entry.relative_path);

    // Get the metadata for the entry
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(PackageEntryValidationError::NotFound)
        }
        Err(e) => return Err(PackageEntryValidationError::GetMetadataFailed(e)),
    };

    // Validate based on the type of path
    match entry.path_type {
        PathType::HardLink => validate_package_hard_link_entry(path, entry, metadata),
        PathType::SoftLink => validate_package_soft_link_entry(path, entry, metadata),
        PathType::Directory => validate_package_directory_entry(path, entry, metadata),
    }
}

/// Determine whether the information in the [`PathsEntry`] matches the file at the specified path.
fn validate_package_hard_link_entry(
    path: PathBuf,
    entry: &PathsEntry,
    metadata: Metadata,
) -> Result<(), PackageEntryValidationError> {
    debug_assert!(entry.path_type == PathType::HardLink);

    // Validate the size of the file
    if let Some(size_in_bytes) = entry.size_in_bytes {
        if size_in_bytes != metadata.len() {
            return Err(PackageEntryValidationError::IncorrectSize(
                size_in_bytes,
                metadata.len(),
            ));
        }
    }

    // Check the SHA256 hash of the file
    if let Some(hash_str) = entry.sha256.as_deref() {
        // Determine the hash of the file on disk
        let hash = compute_file_sha256(&path)?;

        // Convert the hash to bytes.
        let mut expected_hash = <sha2::digest::Output<Sha256>>::default();
        hex::decode_to_slice(hash_str, &mut expected_hash).map_err(|_| {
            PackageEntryValidationError::HashMismatch(hash_str.to_owned(), format!("{:x}", hash))
        })?;

        // Compare the two hashes
        if expected_hash != hash {
            return Err(PackageEntryValidationError::HashMismatch(
                hash_str.to_owned(),
                format!("{:x}", hash),
            ));
        }
    }

    Ok(())
}

/// Determine whether the information in the [`PathsEntry`] matches the symbolic link at the specified
/// path.
fn validate_package_soft_link_entry(
    _path: PathBuf,
    entry: &PathsEntry,
    metadata: Metadata,
) -> Result<(), PackageEntryValidationError> {
    debug_assert!(entry.path_type == PathType::SoftLink);

    if !metadata.is_symlink() {
        return Err(PackageEntryValidationError::ExpectedSymlink);
    }

    // TODO: Validate symlink content

    Ok(())
}

/// Determine whether the information in the [`PathsEntry`] matches the directory at the specified path.
fn validate_package_directory_entry(
    _path: PathBuf,
    entry: &PathsEntry,
    metadata: Metadata,
) -> Result<(), PackageEntryValidationError> {
    debug_assert!(entry.path_type == PathType::Directory);

    if !metadata.is_dir() {
        Err(PackageEntryValidationError::ExpectedDirectory)
    } else {
        Ok(())
    }
}

/// Compute the SHA256 hash of the file at the specified location.
fn compute_file_sha256(path: &Path) -> Result<sha2::digest::Output<sha2::Sha256>, std::io::Error> {
    // Open the file for reading
    let mut file = File::open(path)?;

    // Determine the hash of the file on disk
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;

    Ok(hasher.finalize())
}

#[cfg(test)]
mod test {
    use super::{
        compute_file_sha256, validate_package_files, PackageEntryValidationError,
        PackageValidationError,
    };
    use assert_matches::assert_matches;
    use rattler_conda_types::package::{PathType, PathsJson};
    use rstest::*;
    use std::{
        io::Write,
        path::{Path, PathBuf},
    };

    /// Returns the path to the test data directory
    fn test_data_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
    }

    #[rstest]
    #[case(
        "1234567890",
        "c775e7b757ede630cd0aa1113bd102661ab38829ca52a6422ab782862f268646"
    )]
    #[case(
        "Hello, world!",
        "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
    )]
    fn test_compute_file_sha256(#[case] input: &str, #[case] expected_hash: &str) {
        // Write a known value to a temporary file and verify that the compute hash matches what we would
        // expect.

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test");
        std::fs::write(&file_path, input).unwrap();
        let hash = compute_file_sha256(&file_path).unwrap();

        assert_eq!(format!("{hash:x}"), expected_hash)
    }

    #[rstest]
    #[case::conda_22_9_0("conda-22.9.0-py38haa244fe_2.tar.bz2")]
    #[case::conda_22_11_1("conda-22.11.1-py38haa244fe_1.conda")]
    #[case::pytweening_1_0_4("pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2")]
    #[case::ruff_0_0_171("ruff-0.0.171-py310h298983d_0.conda")]
    fn test_validate_package_files(#[case] package: &str) {
        // Create a temporary directory and extract the given package.
        let temp_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(&test_data_path().join(package), temp_dir.path())
            .unwrap();

        // Validate that the extracted package is correct. Since it's just been extracted this should
        // work.
        let result = validate_package_files(temp_dir.path());
        if let Err(e) = result {
            panic!("{e}");
        }

        // Read the paths.json file and select the first file in the archive.
        let paths = PathsJson::from_path(&temp_dir.path().join("info/paths.json")).unwrap();
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
            validate_package_files(temp_dir.path()),
            Err(PackageValidationError::CorruptedEntry(
                path,
                PackageEntryValidationError::HashMismatch(_, _)
            )) if path == entry.relative_path
        );
    }

    #[rstest]
    #[cfg(unix)]
    #[case::python_3_10_6("linux/python-3.10.6-h2c4edbf_0_cpython.tar.bz2")]
    #[case::cph_test_data_0_0_1("linux/cph_test_data-0.0.1-0.tar.bz2")]
    fn test_validate_package_files_symlink(#[case] package: &str) {
        // Create a temporary directory and extract the given package.
        let temp_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(&test_data_path().join(package), temp_dir.path())
            .unwrap();

        // Validate that the extracted package is correct. Since it's just been extracted this should
        // work.
        let result = validate_package_files(temp_dir.path());
        if let Err(e) = result {
            panic!("{e}");
        }

        // Read the paths.json file and select the first symlink in the archive.
        let paths = PathsJson::from_path(&temp_dir.path().join("info/paths.json")).unwrap();
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
            validate_package_files(temp_dir.path()),
            Err(PackageValidationError::CorruptedEntry(
                path,
                PackageEntryValidationError::ExpectedSymlink
            )) if path == entry.relative_path
        );
    }
}
