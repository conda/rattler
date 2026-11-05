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

use crate::vfs_ops::{ContentSource, Fh, FileAttr, FileKind, VfsError, VfsOps};

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
    VfsManaged(Fh),
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
    /// Open file contents by handle. Values are reference counted so a read
    /// can drop the lock before copying data into the reply, letting the
    /// session's worker threads serve reads concurrently.
    open_files: Mutex<HashMap<u64, Arc<OpenFileContent>>>,
    next_fh: AtomicU64,
    passthrough_enabled: AtomicBool,
    /// Whether the kernel accepts `ENOSYS` from `open` as "this filesystem
    /// needs no open/release" (`FUSE_NO_OPEN_SUPPORT`).
    no_open_supported: AtomicBool,
    /// Whether the mounted tree can never change. Enables kernel-side caching
    /// that would be wrong for a writable overlay: negative lookups and
    /// directory listings.
    read_only: bool,
}

/// How long the kernel may cache entries and attributes. The tree is
/// immutable for a read-only mount and every mutation of a writable mount goes
/// through this filesystem, so the kernel's caches never go stale.
const TTL: Duration = Duration::MAX;

impl<T: VfsOps> FuseAdapter<T> {
    pub fn new(vfs: T) -> Self {
        FuseAdapter {
            vfs,
            open_files: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            passthrough_enabled: AtomicBool::new(false),
            no_open_supported: AtomicBool::new(false),
            read_only: false,
        }
    }

    /// Mark the mounted tree as immutable, see [`FuseAdapter::read_only`].
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    fn store_content(&self, content: OpenFileContent) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        self.open_files
            .lock()
            .unwrap()
            .insert(fh, Arc::new(content));
        fh
    }

    /// Whether `open` should answer `ENOSYS`, which (with
    /// `FUSE_NO_OPEN_SUPPORT`) makes the kernel stop sending `OPEN` and
    /// `RELEASE` for regular files of this mount altogether. Reads then arrive
    /// without a handle and are served by inode. That saves two round trips
    /// per opened file, which dominates when a tool opens thousands of files.
    ///
    /// Only for read-only mounts (writes need handles) and only when kernel
    /// passthrough isn't in use (it is set up per open).
    fn skip_opens(&self) -> bool {
        self.read_only
            && self.no_open_supported.load(Ordering::Relaxed)
            && !self.passthrough_enabled.load(Ordering::Relaxed)
    }
}

/// The number of threads serving FUSE requests. Requests from a single
/// process are mostly sequential, but tools that walk the environment in
/// parallel (and parallel imports in multiple processes) are serialized behind
/// a single thread otherwise.
#[cfg(target_os = "linux")]
fn fuse_worker_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8))
}

impl<T: VfsOps> Filesystem for FuseAdapter<T> {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        if config.add_capabilities(InitFlags::FUSE_PASSTHROUGH).is_ok() {
            self.passthrough_enabled.store(true, Ordering::Relaxed);
            tracing::debug!("FUSE passthrough negotiated");
        }
        if config
            .add_capabilities(InitFlags::FUSE_NO_OPEN_SUPPORT)
            .is_ok()
        {
            self.no_open_supported.store(true, Ordering::Relaxed);
        }
        // Let the kernel cache symlink targets (shared libraries are mostly
        // reached through `libfoo.so -> libfoo.so.1.2` chains) instead of
        // asking for every `readlink` again.
        let _ = config.add_capabilities(InitFlags::FUSE_CACHE_SYMLINKS);
        // Return attributes with directory listings, so walking a tree
        // (`find`, `ls -l`, ripgrep) doesn't need a `LOOKUP` per entry. With
        // `READDIRPLUS_AUTO` the kernel only asks for them when the listing is
        // followed by lookups.
        let _ = config
            .add_capabilities(InitFlags::FUSE_DO_READDIRPLUS | InitFlags::FUSE_READDIRPLUS_AUTO);
        // Allow lookups and readdirs in the same directory to run in parallel.
        let _ = config.add_capabilities(InitFlags::FUSE_PARALLEL_DIROPS);
        // Read ahead as far as the kernel allows: every read is a round trip
        // to this process, and the backing files are local and immutable.
        if let Err(max) = config.set_max_readahead(u32::MAX) {
            let _ = config.set_max_readahead(max);
        }
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.vfs.lookup(parent.0, name) {
            Ok(attr) => {
                let fattr: fuser::FileAttr = attr.into();
                reply.entry(&TTL, &fattr, fuser::Generation(0));
            }
            Err(VfsError::NotFound) if self.read_only => {
                // A successful reply with inode 0 is a negative entry: the
                // kernel caches the absence of `name` for the entry TTL.
                // Interpreters probe many paths that don't exist (Python
                // tries every `sys.path` entry for every import), and
                // without this each probe is a round trip to this process.
                let mut fattr: fuser::FileAttr = FileAttr {
                    ino: 0,
                    size: 0,
                    blocks: 0,
                    atime: std::time::UNIX_EPOCH,
                    mtime: std::time::UNIX_EPOCH,
                    ctime: std::time::UNIX_EPOCH,
                    kind: FileKind::RegularFile,
                    perm: 0,
                    nlink: 0,
                    uid: 0,
                    gid: 0,
                }
                .into();
                fattr.ino = INodeNo(0);
                reply.entry(&TTL, &fattr, fuser::Generation(0));
            }
            Err(e) => reply.error(Errno::from_i32(e.errno())),
        }
    }

    fn opendir(&self, _req: &Request, _ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        // Directory contents of a read-only mount never change, so let the
        // kernel cache listings across opens.
        let flags = if self.read_only {
            FopenFlags::FOPEN_CACHE_DIR | FopenFlags::FOPEN_KEEP_CACHE
        } else {
            FopenFlags::empty()
        };
        reply.opened(FileHandle(0), flags);
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.vfs.getattr(ino.0) {
            Ok(attr) => {
                let fattr: fuser::FileAttr = attr.into();
                reply.attr(&TTL, &fattr);
            }
            Err(e) => reply.error(Errno::from_i32(e.errno())),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        match self.vfs.readlink(ino.0) {
            Ok(target) => reply.data(target.as_os_str().as_encoded_bytes()),
            Err(e) => reply.error(Errno::from_i32(e.errno())),
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
                Err(e) => reply.error(Errno::from_i32(e.errno())),
            }
            return;
        }

        if self.skip_opens() {
            reply.error(Errno::ENOSYS);
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
                                Arc::new(OpenFileContent::Passthrough {
                                    _backing_id: backing_id,
                                }),
                            );
                        }
                        Err(e) => {
                            // Passthrough negotiated but not usable at runtime
                            // (registering a backing file needs CAP_SYS_ADMIN,
                            // so this is the norm for unprivileged users);
                            // disable and fall back to mmap for this and future opens.
                            tracing::debug!(
                                "FUSE passthrough unavailable at runtime ({}), falling back to mmap",
                                e
                            );
                            self.passthrough_enabled.store(false, Ordering::Relaxed);
                            if self.skip_opens() {
                                reply.error(Errno::ENOSYS);
                                return;
                            }
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
            if let Some(content) = removed
                && let OpenFileContent::VfsManaged(vfs_fh) = *content
            {
                self.vfs.release_write(vfs_fh);
            }
        }
        reply.ok();
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock: Option<LockOwner>,
        reply: ReplyData,
    ) {
        // No handle: opens are skipped (see `skip_opens`), serve by inode.
        if fh == FileHandle(0) {
            match self.vfs.read(ino.0, offset, size) {
                Ok(data) => reply.data(&data),
                Err(e) => reply.error(Errno::from_i32(e.errno())),
            }
            return;
        }

        // Clone the handle's content out so the lock isn't held while the
        // data is copied (or produced by the VFS).
        let Some(content) = self.open_files.lock().unwrap().get(&fh.0).cloned() else {
            reply.error(Errno::from_i32(EIO));
            return;
        };

        match &*content {
            OpenFileContent::VfsManaged(vfs_fh) => {
                let vfs_fh = *vfs_fh;
                match self.vfs.read_handle(vfs_fh, offset, size) {
                    Ok(data) => reply.data(&data),
                    Err(e) => reply.error(Errno::from_i32(e.errno())),
                }
            }
            OpenFileContent::InodeRead(ino) => {
                let ino = *ino;
                match self.vfs.read(ino, offset, size) {
                    Ok(data) => reply.data(&data),
                    Err(e) => reply.error(Errno::from_i32(e.errno())),
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
            Err(e) => reply.error(Errno::from_i32(e.errno())),
        }
    }

    fn readdirplus(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: fuser::ReplyDirectoryPlus,
    ) {
        match self.vfs.readdir(ino.0, offset) {
            Ok(entries) => {
                for (i, entry) in entries.into_iter().enumerate() {
                    let Ok(attr) = self.vfs.getattr(entry.ino) else {
                        continue;
                    };
                    let fattr: fuser::FileAttr = attr.into();
                    if reply.add(
                        INodeNo(entry.ino),
                        offset + i as u64 + 1,
                        &entry.name,
                        &TTL,
                        &fattr,
                        fuser::Generation(0),
                    ) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(e) => reply.error(Errno::from_i32(e.errno())),
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
                    &TTL,
                    &fattr,
                    fuser::Generation(0),
                    FileHandle(fh),
                    FopenFlags::empty(),
                );
            }
            Err(e) => reply.error(Errno::from_i32(e.errno())),
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
        let content = self.open_files.lock().unwrap().get(&fh.0).cloned();
        let vfs_fh = if let Some(OpenFileContent::VfsManaged(vfs_fh)) = content.as_deref() {
            *vfs_fh
        } else {
            reply.error(Errno::from_i32(EIO));
            return;
        };
        match self.vfs.write(vfs_fh, offset, data) {
            Ok(written) => reply.written(written),
            Err(e) => reply.error(Errno::from_i32(e.errno())),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.vfs.unlink(parent.0, name) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(Errno::from_i32(e.errno())),
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
                reply.entry(&TTL, &fattr, fuser::Generation(0));
            }
            Err(e) => reply.error(Errno::from_i32(e.errno())),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.vfs.rmdir(parent.0, name) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(Errno::from_i32(e.errno())),
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
            Err(e) => reply.error(Errno::from_i32(e.errno())),
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
                reply.attr(&TTL, &fattr);
            }
            Err(e) => reply.error(Errno::from_i32(e.errno())),
        }
    }
}

// ---------------------------------------------------------------------------
// Mount helper (moved from lib.rs — issue #2576)
// ---------------------------------------------------------------------------

use crate::{Mode, MountConfig, VirtualFS, create_overlay};

/// Mount via FUSE, with optional writable overlay.
///
/// The overlay is retried once if the env hash mismatches (environment updated).
#[cfg(any(target_os = "linux", feature = "fuse"))]
pub(crate) fn mount_fuse(
    vfs: VirtualFS,
    config: &MountConfig,
) -> anyhow::Result<fuser::BackgroundSession> {
    use fuser::{Config as FuserConfig, MountOption, SessionACL};

    let mut fuser_config = FuserConfig::default();
    fuser_config.mount_options = vec![
        MountOption::FSName("conda-packages".to_string()),
        MountOption::NoAtime,
    ];
    if matches!(config.mode, Mode::ReadOnly | Mode::ReadOnlyIfSupported) {
        fuser_config.mount_options.push(MountOption::RO);
    }
    if config.allow_other {
        fuser_config.acl = SessionACL::All;
    }
    #[cfg(target_os = "linux")]
    {
        fuser_config.n_threads = Some(fuse_worker_threads());
    }

    match &config.mode {
        Mode::Writable {
            overlay_dir: Some(overlay_dir),
        } => {
            let overlay = create_overlay(
                vfs,
                overlay_dir,
                &config.env_hash,
                "fuse",
                config.overlay_mismatch,
            )?;
            let adapter = FuseAdapter::new(overlay);
            Ok(fuser::spawn_mount(
                adapter,
                &config.mount_point,
                &fuser_config,
            )?)
        }
        Mode::Writable { overlay_dir: None } => {
            anyhow::bail!(
                "FUSE writable mode requires an overlay directory. Use \
                 MountConfig::new(..).with_writable(overlay_dir) or \
                 MountConfig::new(..) for a read-only mount."
            );
        }
        Mode::ReadOnly | Mode::ReadOnlyIfSupported => {
            let adapter = FuseAdapter::new(vfs).with_read_only(true);
            Ok(fuser::spawn_mount(
                adapter,
                &config.mount_point,
                &fuser_config,
            )?)
        }
    }
}
