//! Unlinking packages from an environment.

use std::{collections::HashSet, io::ErrorKind, path::Path};

use indexmap::IndexSet;
use itertools::Itertools;
use rattler_conda_types::PrefixRecord;

/// Error that can occur while unlinking a package.
#[derive(Debug, thiserror::Error)]
pub enum UnlinkError {
    /// Failed to delete a directory.
    #[error("failed to delete empty directory: {0}")]
    FailedToDeleteDirectory(String, std::io::Error),

    /// Failed to delete a file.
    #[error("failed to delete file: {0}")]
    FailedToDeleteFile(String, std::io::Error),

    /// Failed to read a directory.
    #[error("failed to read directory: {0}")]
    FailedToReadDirectory(String, std::io::Error),
}

/// Completely remove the specified package from the environment.
pub async fn unlink_package(
    target_prefix: &Path,
    prefix_record: &PrefixRecord,
) -> Result<(), UnlinkError> {
    // TODO: Take into account any clobbered files, they need to be restored.

    // Check if package is python noarch
    let is_python_noarch = prefix_record
        .repodata_record
        .package_record
        .noarch
        .is_python();

    let mut directories = HashSet::new();

    // Remove all entries
    for paths in prefix_record.paths_data.paths.iter() {
        match tokio::fs::remove_file(target_prefix.join(&paths.relative_path)).await {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // Simply ignore if the file is already gone.
            }
            Err(e) => {
                return Err(UnlinkError::FailedToDeleteFile(
                    paths.relative_path.to_string_lossy().to_string(),
                    e,
                ))
            }
        }

        if let Some(parent) = paths.relative_path.parent() {
            directories.insert(parent.to_path_buf());
        }
    }

    // Sort the directories by length, so that we delete the deepest directories first.
    let mut directories: IndexSet<_> = directories.into_iter().sorted().collect();
    while let Some(directory) = directories.pop() {
        let directory_path = target_prefix.join(&directory);

        let mut read_dir = directory_path.read_dir().map_err(|e| {
            UnlinkError::FailedToReadDirectory(directory_path.to_string_lossy().to_string(), e)
        })?;

        match read_dir.next().transpose() {
            Ok(None) => {
                // The directory is empty, delete it
                std::fs::remove_dir(&directory_path).map_err(|e| {
                    UnlinkError::FailedToDeleteDirectory(
                        directory_path.to_string_lossy().to_string(),
                        e,
                    )
                })?;
            }

            // Check if the only entry is a `__pycache__` directory
            Ok(Some(entry))
                if is_python_noarch
                    && entry.file_name() == "__pycache__"
                    && read_dir.next().is_none() =>
            {
                // The directory is empty, delete it
                std::fs::remove_dir_all(&directory_path).map_err(|e| {
                    UnlinkError::FailedToDeleteDirectory(
                        directory_path.to_string_lossy().to_string(),
                        e,
                    )
                })?;
            }
            _ => {
                // The directory is not empty which means our parent directory is also not empty,
                // recursively remove the parent directory from the set as well.
                while let Some(parent) = directory.parent() {
                    if !directories.shift_remove(parent) {
                        break;
                    }
                }
            }
        }
    }

    // Remove the conda-meta file
    let conda_meta_path = target_prefix
        .join("conda-meta")
        .join(prefix_record.file_name());

    tokio::fs::remove_file(&conda_meta_path)
        .await
        .map_err(|e| {
            UnlinkError::FailedToDeleteFile(conda_meta_path.to_string_lossy().to_string(), e)
        })?;

    Ok(())
}
