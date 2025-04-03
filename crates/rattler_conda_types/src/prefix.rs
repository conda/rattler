//! Defines the `[Prefix]` struct.

use std::path::{Path, PathBuf};

/// Represents a conda environment prefix (directory).
///
/// On macOS, the directory is excluded from Time Machine upons creation.
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
        fs_err::create_dir_all(&path)?;

        // Exclude from backups on macOS
        #[cfg(target_os = "macos")]
        Self::exclude_from_backups(&path);

        Ok(Self { path })
    }

    #[cfg(target_os = "macos")]
    /// Marks files or directories as excluded from Time Machine on macOS
    ///
    /// This is recommended to prevent derived/temporary files from bloating backups.
    /// <https://github.com/rust-lang/cargo/pull/7192>
    fn exclude_from_backups(path: &Path) {
        use core_foundation::base::TCFType;
        use core_foundation::{number, string, url};
        use std::ptr;

        // For compatibility with 10.7 a string is used instead of global kCFURLIsExcludedFromBackupKey
        let is_excluded_key: Result<string::CFString, _> = "NSURLIsExcludedFromBackupKey".parse();
        let path = url::CFURL::from_path(path, false);
        if let (Some(path), Ok(is_excluded_key)) = (path, is_excluded_key) {
            unsafe {
                url::CFURLSetResourcePropertyForKey(
                    path.as_concrete_TypeRef(),
                    is_excluded_key.as_concrete_TypeRef(),
                    number::kCFBooleanTrue.cast(),
                    ptr::null_mut(),
                );
            }
        }
        // Errors are ignored, since it's an optional feature and failure
        // doesn't prevent Cargo from working
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
