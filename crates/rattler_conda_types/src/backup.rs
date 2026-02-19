//! Utilities for excluding directories from backups.
//!
//! This module provides cross-platform utilities to mark directories as excluded
//! from backups. It supports:
//!
//! - **CACHEDIR.TAG**: A standard file recognized by many backup tools (borg, restic,
//!   duplicity, tar --exclude-caches, etc.). See <https://bford.info/cachedir/>.
//!
//! - **macOS Time Machine**: Uses the `NSURLIsExcludedFromBackupKey` attribute to
//!   exclude directories from Time Machine backups.

use std::io::Write;
use std::path::Path;

/// The standard CACHEDIR.TAG header that identifies a cache directory.
/// See <https://bford.info/cachedir/> for the specification.
const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by rattler.
# For information about cache directory tags, see:
#\thttps://bford.info/cachedir/
";

/// Creates a `CACHEDIR.TAG` file in the specified directory.
///
/// This file signals to backup tools that this directory contains
/// cache/derived data that doesn't need to be backed up.
/// See <https://bford.info/cachedir/> for the specification.
///
/// If the file already exists, this function does nothing.
pub fn create_cachedir_tag(path: &Path) -> Result<(), std::io::Error> {
    let tag_path = path.join("CACHEDIR.TAG");
    if tag_path.exists() {
        return Ok(());
    }

    let mut file = fs_err::File::create(&tag_path)?;
    file.write_all(CACHEDIR_TAG.as_bytes())?;
    Ok(())
}

/// Marks a directory as excluded from Time Machine backups on macOS.
///
/// This is recommended to prevent derived/temporary files from bloating backups.
/// Based on the approach used by Cargo: <https://github.com/rust-lang/cargo/pull/7192>
///
/// Errors are silently ignored since backup exclusion is an optional feature
/// and failure shouldn't prevent the application from working.
#[cfg(target_os = "macos")]
pub fn exclude_from_time_machine(path: &Path) {
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
    // doesn't prevent the application from working
}

/// Excludes a directory from backups using all available methods.
///
/// This function:
/// 1. Creates a `CACHEDIR.TAG` file (cross-platform)
/// 2. Marks the directory as excluded from Time Machine on macOS
///
/// The `CACHEDIR.TAG` creation propagates errors, while Time Machine exclusion
/// silently ignores errors (since it's platform-specific and optional).
pub fn exclude_from_backups(path: &Path) -> Result<(), std::io::Error> {
    create_cachedir_tag(path)?;
    #[cfg(target_os = "macos")]
    exclude_from_time_machine(path);
    Ok(())
}
