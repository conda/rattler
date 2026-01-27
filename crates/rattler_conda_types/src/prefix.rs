//! Defines the `[Prefix]` struct.

use std::path::{Path, PathBuf};

use crate::backup;

/// Represents a conda environment prefix (directory).
///
/// The directory is excluded from backups upon creation (via CACHEDIR.TAG and
/// Time Machine on macOS).
#[derive(Debug, Clone)]
pub struct Prefix {
    path: PathBuf,
}

impl Prefix {
    /// Get the trash directory for the prefix
    pub fn get_trash_dir(&self) -> PathBuf {
        self.path.join(".trash")
    }
}

impl std::ops::Deref for Prefix {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl Prefix {
    /// Create a new prefix, initializing it properly (creating directories, excluding from backups, etc.)
    pub fn create(path: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let path = path.into();

        // Create the target directory if it doesn't exist
        fs_err::create_dir_all(path.join("conda-meta"))?;

        if !path.join("conda-meta/history").exists() {
            // Create an empty history file if it doesn't exist
            fs_err::File::create(path.join("conda-meta/history"))?;
        }

        // Exclude from backups (CACHEDIR.TAG + Time Machine on macOS)
        backup::exclude_from_backups(&path)?;

        Ok(Self { path })
    }

    /// Get a reference to the prefix path
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for Prefix {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}
