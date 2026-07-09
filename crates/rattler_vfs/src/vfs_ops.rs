//! Transport-agnostic virtual filesystem operations.
//!
//! The `VfsOps` trait defines operations that both `VirtualFS` (read-only)
//! and `OverlayFS` (writable) implement. Transport adapters (FUSE, NFS, etc.)
//! are generic over this trait.

use libc::{ENOENT, EROFS};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// File type — transport-agnostic equivalent of `fuser::FileType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    RegularFile,
    Directory,
    Symlink,
}

/// File attributes — transport-agnostic equivalent of `fuser::FileAttr`.
#[derive(Debug, Clone)]
pub struct FileAttr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime: SystemTime,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
    pub kind: FileKind,
    pub perm: u16,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
}

impl FileAttr {
    /// Build a `FileAttr` from OS filesystem metadata, abstracting away
    /// platform differences (blocks, ctime, permissions, uid/gid).
    pub fn from_metadata(metadata: &std::fs::Metadata, ino: u64) -> Self {
        #[cfg(unix)]
        let (blocks, ctime, perm, uid, gid) = {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            use std::time::Duration;
            (
                metadata.blocks(),
                UNIX_EPOCH + Duration::new(metadata.ctime() as u64, metadata.ctime_nsec() as u32),
                (metadata.permissions().mode() & 0o777) as u16,
                metadata.uid(),
                metadata.gid(),
            )
        };
        #[cfg(not(unix))]
        let (blocks, ctime, perm, uid, gid) = {
            (
                (metadata.len() + 511) / 512,
                metadata.modified().unwrap_or(UNIX_EPOCH),
                if metadata.is_dir() {
                    0o755u16
                } else {
                    0o644u16
                },
                0u32,
                0u32,
            )
        };

        let kind = if metadata.is_dir() {
            FileKind::Directory
        } else if metadata.is_symlink() {
            FileKind::Symlink
        } else {
            FileKind::RegularFile
        };

        FileAttr {
            ino,
            size: metadata.len(),
            blocks,
            atime: metadata.accessed().unwrap_or(UNIX_EPOCH),
            mtime: metadata.modified().unwrap_or(UNIX_EPOCH),
            ctime,
            kind,
            perm,
            nlink: 1,
            uid,
            gid,
        }
    }
}

/// Get the current user's UID and GID. Returns `(0, 0)` on non-Unix platforms.
pub fn current_uid_gid() -> (u32, u32) {
    #[cfg(unix)]
    {
        unsafe { (libc::getuid(), libc::getgid()) }
    }
    #[cfg(not(unix))]
    {
        (0, 0)
    }
}

/// Set Unix file permissions cross-platform. On Windows, maps the write bits
/// to the read-only flag.
pub fn set_file_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        let readonly = mode & 0o222 == 0;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_readonly(readonly);
        std::fs::set_permissions(path, perms)
    }
}

/// Hint about how a file's content should be served.
#[derive(Debug)]
pub enum ContentSource {
    /// Unmodified file at this path — adapter can use passthrough, mmap, or
    /// hardlink depending on the transport.
    Direct(PathBuf),
    /// Content requires transformation (prefix replacement, codesign).
    /// The adapter must use `VfsOps::read()` to get bytes.
    Transformed,
    /// Small virtual file (e.g. generated entry point scripts).
    /// The adapter must use `VfsOps::read()` to get bytes.
    Virtual,
}

/// A directory entry returned by `readdir`.
#[derive(Debug, PartialEq)]
pub struct DirEntry {
    pub ino: u64,
    pub kind: FileKind,
    pub name: OsString,
}

/// Transport-agnostic filesystem operations.
///
/// Read operations are required. Write operations default to `EROFS` (read-only
/// filesystem), allowing read-only implementations to skip them.
pub trait VfsOps: Send + Sync + 'static {
    fn lookup(&self, parent: u64, name: &OsStr) -> Result<FileAttr, i32>;
    fn getattr(&self, ino: u64) -> Result<FileAttr, i32>;
    fn readlink(&self, ino: u64) -> Result<PathBuf, i32>;

    /// Read bytes from a file at the given offset. The VFS handles prefix
    /// replacement, codesign, and passthrough transparently.
    fn read(&self, ino: u64, offset: u64, size: u32) -> Result<Vec<u8>, i32>;

    /// Hint about how the adapter should serve this file's content.
    fn content_source(&self, ino: u64) -> Result<ContentSource, i32>;

    fn readdir(&self, ino: u64, offset: u64) -> Result<Vec<DirEntry>, i32>;

    /// Resolve an inode to its virtual path (relative to root).
    /// Used by the overlay to map lower inodes to paths for whiteout checks.
    fn ino_to_path(&self, ino: u64) -> Result<PathBuf, i32> {
        let _ = ino;
        Err(ENOENT)
    }

    // Write operations — default to EROFS for read-only implementations.

    /// Open a file for writing. Returns a write handle.
    fn open_write(&self, _ino: u64) -> Result<u64, i32> {
        Err(EROFS)
    }
    /// Read from a write handle (for files currently open for writing).
    fn read_handle(&self, _fh: u64, _offset: u64, _size: u32) -> Result<Vec<u8>, i32> {
        Err(EROFS)
    }
    fn write(&self, _fh: u64, _offset: u64, _data: &[u8]) -> Result<u32, i32> {
        Err(EROFS)
    }
    fn release_write(&self, _fh: u64) {}

    fn create(&self, _parent: u64, _name: &OsStr, _mode: u32) -> Result<(FileAttr, u64), i32> {
        Err(EROFS)
    }
    fn unlink(&self, _parent: u64, _name: &OsStr) -> Result<(), i32> {
        Err(EROFS)
    }
    fn mkdir(&self, _parent: u64, _name: &OsStr, _mode: u32) -> Result<FileAttr, i32> {
        Err(EROFS)
    }
    fn rmdir(&self, _parent: u64, _name: &OsStr) -> Result<(), i32> {
        Err(EROFS)
    }
    fn rename(
        &self,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        _flags: u32,
    ) -> Result<(), i32> {
        Err(EROFS)
    }
    fn setattr(&self, _ino: u64, _size: Option<u64>, _mode: Option<u32>) -> Result<FileAttr, i32> {
        Err(EROFS)
    }
}
