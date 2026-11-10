//! Transport-agnostic virtual filesystem operations.
//!
//! The `VfsOps` trait defines operations that both `VirtualFS` (read-only)
//! and `OverlayFS` (writable) implement. Transport adapters (FUSE, NFS, etc.)
//! are generic over this trait.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Error returned by [`VfsOps`] operations.
///
/// A small, transport-agnostic error enum rather than a bare `errno` `i32`, so
/// the VFS layer names its failure modes explicitly. Transport adapters convert
/// it to their own wire error (NFS `nfsstat3`, FUSE `Errno`, `ProjFS` `HRESULT`)
/// — usually via [`VfsError::errno`] / `From<VfsError> for i32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    /// No such file or directory (`ENOENT`).
    NotFound,
    /// Permission denied (`EACCES`).
    PermissionDenied,
    /// Not a directory (`ENOTDIR`).
    NotADirectory,
    /// Is a directory (`EISDIR`).
    IsADirectory,
    /// Read-only filesystem (`EROFS`).
    ReadOnly,
    /// File exists (`EEXIST`).
    AlreadyExists,
    /// Directory not empty (`ENOTEMPTY`).
    NotEmpty,
    /// No space left on device (`ENOSPC`).
    NoSpace,
    /// Invalid argument (`EINVAL`) — e.g. an unsafe wire filename.
    InvalidArgument,
    /// Operation not permitted (`EPERM`).
    NotPermitted,
    /// Catch-all I/O error (`EIO`).
    Io,
}

impl VfsError {
    /// The POSIX `errno` this error maps to.
    pub fn errno(self) -> i32 {
        match self {
            Self::NotFound => libc::ENOENT,
            Self::PermissionDenied => libc::EACCES,
            Self::NotADirectory => libc::ENOTDIR,
            Self::IsADirectory => libc::EISDIR,
            Self::ReadOnly => libc::EROFS,
            Self::AlreadyExists => libc::EEXIST,
            Self::NotEmpty => libc::ENOTEMPTY,
            Self::NoSpace => libc::ENOSPC,
            Self::InvalidArgument => libc::EINVAL,
            Self::NotPermitted => libc::EPERM,
            Self::Io => libc::EIO,
        }
    }

    /// Map a POSIX `errno` back to a [`VfsError`], collapsing anything unknown
    /// to [`VfsError::Io`].
    pub fn from_errno(errno: i32) -> Self {
        match errno {
            libc::ENOENT => Self::NotFound,
            libc::EACCES => Self::PermissionDenied,
            libc::ENOTDIR => Self::NotADirectory,
            libc::EISDIR => Self::IsADirectory,
            libc::EROFS => Self::ReadOnly,
            libc::EEXIST => Self::AlreadyExists,
            libc::ENOTEMPTY => Self::NotEmpty,
            libc::ENOSPC => Self::NoSpace,
            libc::EINVAL => Self::InvalidArgument,
            libc::EPERM => Self::NotPermitted,
            _ => Self::Io,
        }
    }
}

impl From<VfsError> for i32 {
    fn from(e: VfsError) -> Self {
        e.errno()
    }
}

impl std::fmt::Display for VfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?} (errno {})", self.errno())
    }
}

impl std::error::Error for VfsError {}

/// Convenience alias for VFS operation results.
pub type VfsResult<T> = Result<T, VfsError>;

/// An opaque write-handle token returned by [`VfsOps::open_write`] /
/// [`VfsOps::create`] and passed back to [`VfsOps::write`],
/// [`VfsOps::read_handle`], and [`VfsOps::release_write`].
///
/// A newtype rather than a bare `u64` so a file handle can't be silently
/// swapped for an inode (both would otherwise be `u64`) — the NFS adapter in
/// particular keeps an inode→handle map where mixing the two would be a subtle
/// bug. Inodes are deliberately left as `u64`: they're used as array indices,
/// map keys, and in the overlay's `UPPER_INODE_BASE` partition arithmetic,
/// where a newtype would add noise without preventing a real error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fh(pub u64);

impl Fh {
    /// The underlying handle value.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for Fh {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<Fh> for u64 {
    fn from(fh: Fh) -> Self {
        fh.0
    }
}

impl std::fmt::Display for Fh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

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
    fn lookup(&self, parent: u64, name: &OsStr) -> VfsResult<FileAttr>;
    fn getattr(&self, ino: u64) -> VfsResult<FileAttr>;
    fn readlink(&self, ino: u64) -> VfsResult<PathBuf>;

    /// Read bytes from a file at the given offset. The VFS handles prefix
    /// replacement, codesign, and passthrough transparently.
    fn read(&self, ino: u64, offset: u64, size: u32) -> VfsResult<Vec<u8>>;

    /// Hint about how the adapter should serve this file's content.
    fn content_source(&self, ino: u64) -> VfsResult<ContentSource>;

    fn readdir(&self, ino: u64, offset: u64) -> VfsResult<Vec<DirEntry>>;

    /// Resolve an inode to its virtual path (relative to root).
    /// Used by the overlay to map lower inodes to paths for whiteout checks.
    fn ino_to_path(&self, ino: u64) -> VfsResult<PathBuf> {
        let _ = ino;
        Err(VfsError::NotFound)
    }

    // Write operations — default to read-only (`EROFS`) for read-only impls.

    /// Open a file for writing. Returns a write handle.
    fn open_write(&self, _ino: u64) -> VfsResult<Fh> {
        Err(VfsError::ReadOnly)
    }
    /// Read from a write handle (for files currently open for writing).
    fn read_handle(&self, _fh: Fh, _offset: u64, _size: u32) -> VfsResult<Vec<u8>> {
        Err(VfsError::ReadOnly)
    }
    fn write(&self, _fh: Fh, _offset: u64, _data: &[u8]) -> VfsResult<u32> {
        Err(VfsError::ReadOnly)
    }
    fn release_write(&self, _fh: Fh) {}

    fn create(&self, _parent: u64, _name: &OsStr, _mode: u32) -> VfsResult<(FileAttr, Fh)> {
        Err(VfsError::ReadOnly)
    }
    fn unlink(&self, _parent: u64, _name: &OsStr) -> VfsResult<()> {
        Err(VfsError::ReadOnly)
    }
    fn mkdir(&self, _parent: u64, _name: &OsStr, _mode: u32) -> VfsResult<FileAttr> {
        Err(VfsError::ReadOnly)
    }
    fn rmdir(&self, _parent: u64, _name: &OsStr) -> VfsResult<()> {
        Err(VfsError::ReadOnly)
    }
    fn rename(
        &self,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        _flags: u32,
    ) -> VfsResult<()> {
        Err(VfsError::ReadOnly)
    }
    fn setattr(&self, _ino: u64, _size: Option<u64>, _mode: Option<u32>) -> VfsResult<FileAttr> {
        Err(VfsError::ReadOnly)
    }
}
