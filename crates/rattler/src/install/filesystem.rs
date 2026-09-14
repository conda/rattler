//! Filesystem-specific policy and capability probes used while linking package contents.

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
#[cfg(unix)]
use std::{
    collections::HashMap,
    os::unix::fs::MetadataExt,
    sync::{LazyLock, Mutex},
};

use fs_err::tokio as tokio_fs;
use rattler_conda_types::prefix::Prefix;
#[cfg(windows)]
use windows_sys::Win32::{
    Storage::FileSystem::{GetVolumeInformationW, GetVolumePathNameW},
    System::Diagnostics::Debug::{GetThreadErrorMode, SEM_FAILCRITICALERRORS, SetThreadErrorMode},
};

/// Whether directory creation completes before or overlaps file linking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DirectoryCreationMode {
    Barrier,
    Overlap,
}

impl DirectoryCreationMode {
    #[cfg(not(windows))]
    pub(super) fn for_target(_root: &Path) -> Self {
        if cfg!(target_os = "macos") {
            Self::Barrier
        } else {
            Self::Overlap
        }
    }

    /// `ReFS` benefits from overlapping directory creation and file linking,
    /// while other or unknown Windows filesystems use a directory-creation barrier.
    #[cfg(windows)]
    pub(super) fn for_target(root: &Path) -> Self {
        let Ok(absolute) = std::path::absolute(root) else {
            return Self::Barrier;
        };
        let Some(_error_mode) = ErrorModeGuard::suppress_critical_errors() else {
            return Self::Barrier;
        };
        let Some(existing) = absolute.ancestors().find(|path| path.exists()) else {
            return Self::Barrier;
        };
        let existing = existing
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut volume_root = [0u16; 260];
        // SAFETY: both buffers remain valid for the duration of the call. The
        // input is null-terminated and the writable buffer length is exact.
        let success = unsafe {
            GetVolumePathNameW(
                existing.as_ptr(),
                volume_root.as_mut_ptr(),
                volume_root.len() as u32,
            )
        };
        if success == 0 {
            return Self::Barrier;
        }

        let mut filesystem_name = [0u16; 16];
        // SAFETY: the volume-root buffer was initialized and null-terminated by
        // GetVolumePathNameW. The optional output pointers are null, and the
        // writable filesystem-name buffer length matches the value passed.
        let success = unsafe {
            GetVolumeInformationW(
                volume_root.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                filesystem_name.as_mut_ptr(),
                filesystem_name.len() as u32,
            )
        };
        if success != 0 && filesystem_name.starts_with(&[0x52, 0x65, 0x46, 0x53, 0]) {
            Self::Overlap
        } else {
            Self::Barrier
        }
    }
}

#[cfg(windows)]
struct ErrorModeGuard(u32);

#[cfg(windows)]
impl ErrorModeGuard {
    fn suppress_critical_errors() -> Option<Self> {
        // SAFETY: these functions access error-mode state belonging to
        // the current thread. The previous mode is saved by value.
        let previous = unsafe { GetThreadErrorMode() };
        // SAFETY: the null output pointer is explicitly supported.
        let success =
            unsafe { SetThreadErrorMode(previous | SEM_FAILCRITICALERRORS, std::ptr::null_mut()) };
        (success != 0).then_some(Self(previous))
    }
}

#[cfg(windows)]
impl Drop for ErrorModeGuard {
    fn drop(&mut self) {
        // SAFETY: this restores the value read from this thread before
        // the guard was created. The null output pointer is supported.
        let _ = unsafe { SetThreadErrorMode(self.0, std::ptr::null_mut()) };
    }
}

/// Returns true if it is possible to create symlinks in the target directory.
pub(super) fn can_create_symlinks_sync(target_dir: &Prefix) -> bool {
    let uuid = uuid::Uuid::new_v4();
    let symlink_path = target_dir.path().join(format!("symtest_{uuid}"));
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file("./", &symlink_path);
    #[cfg(unix)]
    let result = fs_err::os::unix::fs::symlink("./", &symlink_path);
    match result {
        Ok(_) => {
            if let Err(e) = fs_err::remove_file(&symlink_path) {
                tracing::warn!(
                    "failed to delete temporary file '{}': {e}",
                    symlink_path.display()
                );
            }
            true
        }
        Err(e) => {
            tracing::debug!(
                "failed to create symlink in target directory: {e}. Disabling use of symlinks."
            );
            false
        }
    }
}

/// Returns true if it is possible to create symlinks in the target directory.
pub(super) async fn can_create_symlinks(target_dir: &Prefix) -> bool {
    let uuid = uuid::Uuid::new_v4();
    let symlink_path = target_dir.path().join(format!("symtest_{uuid}"));
    #[cfg(windows)]
    let result = tokio_fs::symlink_file("./", &symlink_path).await;
    #[cfg(unix)]
    let result = tokio_fs::symlink("./", &symlink_path).await;
    match result {
        Ok(_) => {
            if let Err(e) = tokio_fs::remove_file(&symlink_path).await {
                tracing::warn!(
                    "failed to delete temporary file '{}': {e}",
                    symlink_path.display()
                );
            }
            true
        }
        Err(e) => {
            tracing::debug!(
                "failed to create symlink in target directory: {e}. Disabling use of symlinks."
            );
            false
        }
    }
}

/// Returns true if it is possible to create hard links from the target
/// directory to the package cache directory.
pub(super) async fn can_create_hardlinks(target_dir: &Prefix, package_dir: &Path) -> bool {
    paths_have_same_filesystem(target_dir, package_dir).await
}

/// Returns true if it is possible to create hard links from the target
/// directory to the package cache directory.
pub(super) fn can_create_hardlinks_sync(target_dir: &Prefix, package_dir: &Path) -> bool {
    paths_have_same_filesystem_sync(target_dir.path(), package_dir)
}

/// Returns true if it is possible to create reflinks (copy-on-write clones)
/// from the package cache directory to the target directory.
///
/// [`reflink_copy::check_reflink_support`] only returns a definitive answer on
/// Windows; on all other platforms it returns `Unknown`. Guessing wrong is
/// expensive: every failed reflink attempt creates the destination file,
/// issues the clone ioctl, and removes the destination again before falling
/// back to a hard link or copy. To avoid paying that cost for every file we
/// probe reflink support once per filesystem and cache the result for the
/// lifetime of the process.
pub(super) fn can_create_reflinks_sync(
    target_dir: &Prefix,
    package_dir: &Path,
    fallback: bool,
) -> bool {
    match reflink_copy::check_reflink_support(package_dir, target_dir.path()) {
        Ok(reflink_copy::ReflinkSupport::Supported) => true,
        Ok(reflink_copy::ReflinkSupport::NotSupported) | Err(_) => false,
        Ok(reflink_copy::ReflinkSupport::Unknown) => {
            probe_reflink_support(target_dir.path(), package_dir, fallback)
        }
    }
}

/// Determines whether reflinks work between the two paths by performing a
/// trial reflink in the target directory. The result is cached per device so
/// the probe runs at most once per filesystem.
#[cfg(unix)]
fn probe_reflink_support(target_dir: &Path, package_dir: &Path, _fallback: bool) -> bool {
    static REFLINK_SUPPORT_CACHE: LazyLock<Mutex<HashMap<u64, bool>>> =
        LazyLock::new(Mutex::default);

    let (Ok(target_meta), Ok(package_meta)) = (
        std::fs::metadata(target_dir),
        std::fs::metadata(package_dir),
    ) else {
        return false;
    };

    // Reflinks cannot cross filesystem boundaries.
    if target_meta.dev() != package_meta.dev() {
        return false;
    }

    let mut cache = REFLINK_SUPPORT_CACHE.lock().unwrap();
    if let Some(&supported) = cache.get(&target_meta.dev()) {
        return supported;
    }

    // Probe by reflinking a small file inside a temporary directory in the
    // target directory. Since the package directory resides on the same
    // filesystem the result also holds for cache-to-prefix reflinks. The
    // temporary directory is removed again when it goes out of scope, even if
    // the probe fails halfway through.
    let supported = tempfile::Builder::new()
        .prefix(".refprobe-")
        .tempdir_in(target_dir)
        .is_ok_and(|probe_dir| {
            let probe_src = probe_dir.path().join("src");
            let probe_dst = probe_dir.path().join("dst");
            std::fs::write(&probe_src, b"reflink probe").is_ok()
                && reflink_copy::reflink(&probe_src, &probe_dst).is_ok()
        });

    if !supported {
        tracing::debug!(
            "filesystem of '{}' does not support reflinks, disabling reflink usage",
            target_dir.display()
        );
    }

    cache.insert(target_meta.dev(), supported);
    supported
}

/// On platforms where we cannot cheaply identify the filesystem we keep the
/// previous heuristic.
#[cfg(not(unix))]
fn probe_reflink_support(_target_dir: &Path, _package_dir: &Path, fallback: bool) -> bool {
    fallback
}

/// Returns true if two paths share the same filesystem
#[cfg(unix)]
async fn paths_have_same_filesystem(a: &Prefix, b: &Path) -> bool {
    match tokio::join!(tokio_fs::metadata(a.path()), tokio_fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        _ => false,
    }
}

/// Returns true if two paths share the same filesystem
#[cfg(unix)]
fn paths_have_same_filesystem_sync(a: &Path, b: &Path) -> bool {
    let a = std::fs::metadata(a);
    let b = std::fs::metadata(b);
    match (a, b) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        _ => false,
    }
}

/// Returns true if two paths share the same filesystem
#[cfg(not(unix))]
async fn paths_have_same_filesystem(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a.components().next() == b.components().next(),
        _ => false,
    }
}

/// Returns true if two paths share the same filesystem
#[cfg(not(unix))]
fn paths_have_same_filesystem_sync(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a.components().next() == b.components().next(),
        _ => false,
    }
}
