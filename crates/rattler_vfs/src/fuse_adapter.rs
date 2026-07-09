//! FUSE transport adapter.
//!
//! Thin layer that maps `fuser::Filesystem` callbacks to `VfsOps` trait methods.
//! Handles FUSE-specific concerns: capability negotiation, passthrough, reply types.

use fuser::{
    Errno, FileHandle, Filesystem, FopenFlags, INodeNo, InitFlags, KernelConfig, LockOwner,
    OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyXattr,
    Request,
};
use libc::EIO;
use memmap2::Mmap;
use std::{
    collections::HashMap,
    ffi::OsStr,
    fs::File,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::vfs_ops::{ContentSource, FileAttr, FileKind, VfsOps};

// ---------------------------------------------------------------------------
// Conversions between crate-local types and fuser types
// ---------------------------------------------------------------------------

impl From<FileKind> for fuser::FileType {
    fn from(kind: FileKind) -> fuser::FileType {
        match kind {
            FileKind::RegularFile => fuser::FileType::RegularFile,
            FileKind::Directory => fuser::FileType::Directory,
            FileKind::Symlink => fuser::FileType::Symlink,
        }
    }
}

impl From<FileAttr> for fuser::FileAttr {
    fn from(attr: FileAttr) -> fuser::FileAttr {
        fuser::FileAttr {
            ino: INodeNo(attr.ino),
            size: attr.size,
            blocks: attr.blocks,
            atime: attr.atime,
            mtime: attr.mtime,
            ctime: attr.ctime,
            crtime: attr.atime,
            kind: attr.kind.into(),
            perm: attr.perm,
            nlink: attr.nlink,
            uid: attr.uid,
            gid: attr.gid,
            rdev: 0,
            flags: 0,
            blksize: 512,
        }
    }
}

/// Content of an open file managed by the adapter.
enum OpenFileContent {
    /// Materialized content from the VFS (e.g. after prefix replacement).
    Materialized(Vec<u8>),
    /// Raw mmap of the cache file (no prefix replacement needed).
    Mapped(Mmap),
    /// Kernel handles reads directly via the backing fd (Linux 6.9+).
    /// The `BackingId` must stay alive until `release()`.
    Passthrough { _backing_id: Arc<fuser::BackingId> },
    /// VFS-managed handle (overlay writable files). Reads/writes/release
    /// must be delegated back to the VFS using this key.
    VfsManaged(u64),
    /// Inode-based reads for transformed/virtual content. Each `read()`
    /// call delegates to `VfsOps::read(ino, offset, size)`.
    InodeRead(u64),
}

impl OpenFileContent {
    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Materialized(buf) => Some(buf),
            Self::Mapped(mmap) => Some(mmap.as_ref()),
            Self::Passthrough { .. } | Self::VfsManaged(_) | Self::InodeRead(_) => None,
        }
    }
}

/// FUSE transport adapter, generic over any `VfsOps` implementation.
pub struct FuseAdapter<T: VfsOps> {
    vfs: T,
    open_files: Mutex<HashMap<u64, OpenFileContent>>,
    next_fh: AtomicU64,
    passthrough_enabled: AtomicBool,
}

impl<T: VfsOps> FuseAdapter<T> {
    pub fn new(vfs: T) -> Self {
        FuseAdapter {
            vfs,
            open_files: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            passthrough_enabled: AtomicBool::new(false),
        }
    }

    fn store_content(&self, content: OpenFileContent) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        self.open_files.lock().unwrap().insert(fh, content);
        fh
    }
}

impl<T: VfsOps> Filesystem for FuseAdapter<T> {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        if config.add_capabilities(InitFlags::FUSE_PASSTHROUGH).is_ok() {
            self.passthrough_enabled.store(true, Ordering::Relaxed);
            tracing::info!("FUSE passthrough enabled");
        }
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.vfs.lookup(parent.0, name) {
            Ok(attr) => {
                let fattr: fuser::FileAttr = attr.into();
                reply.entry(&Duration::MAX, &fattr, fuser::Generation(0));
            }
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.vfs.getattr(ino.0) {
            Ok(attr) => {
                let fattr: fuser::FileAttr = attr.into();
                reply.attr(&Duration::MAX, &fattr);
            }
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        match self.vfs.readlink(ino.0) {
            Ok(target) => reply.data(target.as_os_str().as_encoded_bytes()),
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let write = matches!(
            flags.acc_mode(),
            fuser::OpenAccMode::O_WRONLY | fuser::OpenAccMode::O_RDWR
        );

        if write {
            // Write path: delegate to VFS open_write
            match self.vfs.open_write(ino.0) {
                Ok(vfs_fh) => {
                    let fh = self.store_content(OpenFileContent::VfsManaged(vfs_fh));
                    reply.opened(FileHandle(fh), FopenFlags::empty());
                }
                Err(e) => reply.error(Errno::from_i32(e)),
            }
            return;
        }

        // Read path: use content_source to decide strategy
        match self.vfs.content_source(ino.0) {
            Ok(ContentSource::Direct(path)) => {
                if self.passthrough_enabled.load(Ordering::Relaxed) {
                    let file = match File::open(&path) {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::warn!("passthrough open failed for {}: {}", path.display(), e);
                            reply.error(Errno::from_i32(EIO));
                            return;
                        }
                    };
                    match reply.open_backing(&file) {
                        Ok(id) => {
                            let backing_id = Arc::new(id);
                            let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
                            reply.opened_passthrough(
                                FileHandle(fh),
                                FopenFlags::FOPEN_KEEP_CACHE,
                                &backing_id,
                            );
                            self.open_files.lock().unwrap().insert(
                                fh,
                                OpenFileContent::Passthrough {
                                    _backing_id: backing_id,
                                },
                            );
                        }
                        Err(e) => {
                            // Passthrough negotiated but not usable at runtime;
                            // disable and fall back to mmap for this and future opens.
                            tracing::warn!(
                                "FUSE passthrough unavailable at runtime ({}), falling back to mmap",
                                e
                            );
                            self.passthrough_enabled.store(false, Ordering::Relaxed);
                            let mmap = match unsafe { Mmap::map(&file) } {
                                Ok(m) => m,
                                Err(e) => {
                                    tracing::warn!("failed to mmap {}: {}", path.display(), e);
                                    reply.error(Errno::from_i32(EIO));
                                    return;
                                }
                            };
                            let fh = self.store_content(OpenFileContent::Mapped(mmap));
                            reply.opened(FileHandle(fh), FopenFlags::FOPEN_KEEP_CACHE);
                        }
                    };
                } else {
                    // Passthrough not available — mmap and serve normally
                    let file = match File::open(&path) {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::warn!("failed to open {}: {}", path.display(), e);
                            reply.error(Errno::from_i32(EIO));
                            return;
                        }
                    };
                    let mmap = match unsafe { Mmap::map(&file) } {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!("failed to mmap {}: {}", path.display(), e);
                            reply.error(Errno::from_i32(EIO));
                            return;
                        }
                    };
                    let fh = self.store_content(OpenFileContent::Mapped(mmap));
                    reply.opened(FileHandle(fh), FopenFlags::FOPEN_KEEP_CACHE);
                }
            }
            Ok(ContentSource::Transformed | ContentSource::Virtual) => {
                // Inode-based reads — store the inode so read() can call VFS
                let fh = self.store_content(OpenFileContent::InodeRead(ino.0));
                reply.opened(FileHandle(fh), FopenFlags::FOPEN_KEEP_CACHE);
            }
            Err(e) => {
                // Directories/symlinks — open with empty content
                let _ = e;
                let fh = self.store_content(OpenFileContent::Materialized(vec![]));
                reply.opened(FileHandle(fh), FopenFlags::FOPEN_KEEP_CACHE);
            }
        }
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        if fh != FileHandle(0) {
            let removed = self.open_files.lock().unwrap().remove(&fh.0);
            // Propagate release to VFS for overlay-managed handles
            if let Some(OpenFileContent::VfsManaged(vfs_fh)) = removed {
                self.vfs.release_write(vfs_fh);
            }
        }
        reply.ok();
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let lock = self.open_files.lock().unwrap();
        let Some(content) = lock.get(&fh.0) else {
            reply.error(Errno::from_i32(EIO));
            return;
        };

        match content {
            OpenFileContent::VfsManaged(vfs_fh) => {
                let vfs_fh = *vfs_fh;
                drop(lock); // release adapter lock before calling into VFS
                match self.vfs.read_handle(vfs_fh, offset, size) {
                    Ok(data) => reply.data(&data),
                    Err(e) => reply.error(Errno::from_i32(e)),
                }
            }
            OpenFileContent::InodeRead(ino) => {
                let ino = *ino;
                drop(lock); // release adapter lock before calling into VFS
                match self.vfs.read(ino, offset, size) {
                    Ok(data) => reply.data(&data),
                    Err(e) => reply.error(Errno::from_i32(e)),
                }
            }
            _ => match content.as_bytes() {
                Some(data) => {
                    let start = (offset as usize).min(data.len());
                    let end = (start + size as usize).min(data.len());
                    reply.data(&data[start..end]);
                }
                None => {
                    // Passthrough — kernel handles reads, shouldn't reach here
                    reply.error(Errno::from_i32(EIO));
                }
            },
        }
    }

    fn getxattr(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        reply.error(Errno::NO_XATTR);
    }

    fn listxattr(&self, _req: &Request, _ino: INodeNo, size: u32, reply: ReplyXattr) {
        if size == 0 {
            reply.size(0);
        } else {
            reply.data(&[]);
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        match self.vfs.readdir(ino.0, offset) {
            Ok(entries) => {
                for (i, entry) in entries.into_iter().enumerate() {
                    let ftype: fuser::FileType = entry.kind.into();
                    if reply.add(
                        INodeNo(entry.ino),
                        offset + i as u64 + 1,
                        ftype,
                        &entry.name,
                    ) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    // Write operations — delegate to VfsOps (EROFS for read-only, overlay handles them)

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _flags: i32,
        reply: fuser::ReplyCreate,
    ) {
        match self.vfs.create(parent.0, name, mode) {
            Ok((attr, vfs_fh)) => {
                let fattr: fuser::FileAttr = attr.into();
                let fh = self.store_content(OpenFileContent::VfsManaged(vfs_fh));
                reply.created(
                    &Duration::MAX,
                    &fattr,
                    fuser::Generation(0),
                    FileHandle(fh),
                    FopenFlags::empty(),
                );
            }
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: fuser::WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: fuser::ReplyWrite,
    ) {
        let vfs_fh = if let Some(OpenFileContent::VfsManaged(vfs_fh)) =
            self.open_files.lock().unwrap().get(&fh.0)
        {
            *vfs_fh
        } else {
            reply.error(Errno::from_i32(EIO));
            return;
        };
        match self.vfs.write(vfs_fh, offset, data) {
            Ok(written) => reply.written(written),
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.vfs.unlink(parent.0, name) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        match self.vfs.mkdir(parent.0, name, mode) {
            Ok(attr) => {
                let fattr: fuser::FileAttr = attr.into();
                reply.entry(&Duration::MAX, &fattr, fuser::Generation(0));
            }
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.vfs.rmdir(parent.0, name) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: fuser::RenameFlags,
        reply: ReplyEmpty,
    ) {
        match self
            .vfs
            .rename(parent.0, name, newparent.0, newname, flags.bits())
        {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        match self.vfs.setattr(ino.0, size, mode) {
            Ok(attr) => {
                let fattr: fuser::FileAttr = attr.into();
                reply.attr(&Duration::MAX, &fattr);
            }
            Err(e) => reply.error(Errno::from_i32(e)),
        }
    }
}
