//! Helpers to check that what rattler writes into a cache directory can be
//! read by the other users the directory is shared with.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
};

/// How the users that share a cache directory are given access to it.
#[derive(Debug, Clone, Copy)]
pub enum SharedAccess {
    /// A umask of `002`, which gives the group the same access as the owner.
    Umask,

    /// A default ACL on the cache directory that gives the group access, while
    /// the umask (`077`) on its own would keep everything private.
    DefaultAcl,
}

/// The umask is process-wide, so tests that change it must not overlap.
static UMASK_LOCK: Mutex<()> = Mutex::new(());

/// A temporary cache directory that is shared with the group of the current
/// user, either through the umask or through a default ACL.
///
/// The umask of the process is changed while this value is alive. Tests
/// that use it are serialized with each other, but other tests in the same
/// process may still create files with this umask.
pub struct SharedCacheDir {
    dir: tempfile::TempDir,
    previous_umask: libc::mode_t,
    _lock: MutexGuard<'static, ()>,
}

impl SharedCacheDir {
    /// Creates the cache directory. Returns `None` if `access` is not
    /// supported by the platform or by the filesystem of the temporary
    /// directory.
    pub fn new(access: SharedAccess) -> Option<Self> {
        let lock = UMASK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        fs_err::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o770)).unwrap();
        let umask = match access {
            SharedAccess::Umask => 0o002,
            SharedAccess::DefaultAcl => {
                if let Err(err) = set_default_group_acl(dir.path()) {
                    eprintln!("skipping test, cannot set a default ACL: {err}");
                    return None;
                }
                0o077
            }
        };
        // SAFETY: `umask` cannot fail.
        let previous_umask = unsafe { libc::umask(umask) };
        Some(Self {
            dir,
            previous_umask,
            _lock: lock,
        })
    }

    /// The path of the cache directory.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Asserts that the group can read every file and enter every directory
    /// inside the cache directory.
    pub fn assert_group_readable(&self) {
        let mut entries = Vec::new();
        collect_entries(self.path(), &mut entries);
        assert!(
            entries.iter().any(|(_, metadata)| metadata.is_file()),
            "no files were written to {}",
            self.path().display()
        );

        let unreadable = entries
            .iter()
            .filter(|(_, metadata)| {
                let required = if metadata.is_dir() { 0o050 } else { 0o040 };
                metadata.permissions().mode() & required != required
            })
            .map(|(path, metadata)| {
                format!(
                    "{:o} {}",
                    metadata.permissions().mode() & 0o777,
                    path.strip_prefix(self.path()).unwrap().display()
                )
            })
            .collect::<Vec<_>>();
        assert!(
            unreadable.is_empty(),
            "the group cannot read:\n{}",
            unreadable.join("\n")
        );
    }
}

impl Drop for SharedCacheDir {
    fn drop(&mut self) {
        // SAFETY: `umask` cannot fail.
        unsafe { libc::umask(self.previous_umask) };
    }
}

/// Recursively collects everything below `dir`, except symbolic links.
fn collect_entries(dir: &Path, entries: &mut Vec<(PathBuf, std::fs::Metadata)>) {
    for entry in fs_err::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let metadata = fs_err::symlink_metadata(&path).unwrap();
        if metadata.is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_entries(&path, entries);
        }
        entries.push((path, metadata));
    }
}

/// Sets the equivalent of `setfacl -d -m u::rwx,g::rwx,g:<gid>:rwx,m::rwx,o::-`
/// on `dir`, where `<gid>` is the effective group of the current process.
#[cfg(target_os = "linux")]
fn set_default_group_acl(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    // See `linux/posix_acl_xattr.h`. Entries are sorted by tag.
    const VERSION: u32 = 2;
    const USER_OBJ: u16 = 0x01;
    const GROUP_OBJ: u16 = 0x04;
    const GROUP: u16 = 0x08;
    const MASK: u16 = 0x10;
    const OTHER: u16 = 0x20;
    const UNDEFINED_ID: u32 = u32::MAX;

    // SAFETY: `getegid` cannot fail.
    let gid = unsafe { libc::getegid() };
    let entries = [
        (USER_OBJ, 0o7u16, UNDEFINED_ID),
        (GROUP_OBJ, 0o7, UNDEFINED_ID),
        (GROUP, 0o7, gid),
        (MASK, 0o7, UNDEFINED_ID),
        (OTHER, 0o0, UNDEFINED_ID),
    ];
    let mut value = VERSION.to_le_bytes().to_vec();
    for (tag, perm, id) in entries {
        value.extend(tag.to_le_bytes());
        value.extend(perm.to_le_bytes());
        value.extend(id.to_le_bytes());
    }

    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).unwrap();
    // SAFETY: both strings are nul-terminated and `value` outlives the call.
    let result = unsafe {
        libc::setxattr(
            path.as_ptr(),
            c"system.posix_acl_default".as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn set_default_group_acl(_dir: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}
