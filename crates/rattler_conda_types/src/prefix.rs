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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefix_create_generates_cachedir_tag() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = Prefix::create(temp_dir.path()).unwrap();

        // Check that CACHEDIR.TAG exists
        let cachedir_tag_path = prefix.path().join("CACHEDIR.TAG");
        assert!(cachedir_tag_path.exists(), "CACHEDIR.TAG should be created");

        // Check that it has the correct signature
        let content = std::fs::read_to_string(&cachedir_tag_path).unwrap();
        assert!(
            content.starts_with("Signature: 8a477f597d28d172789f06886806bc55"),
            "CACHEDIR.TAG should have the correct signature"
        );
    }
}
