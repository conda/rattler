//! NFS transport adapter.
//!
//! Thin layer that maps NFS3 callbacks to `VfsOps` trait methods.
//! Implements `NfsReadFileSystem` for read-only mounts and `NfsFileSystem`
//! for writable overlay mounts. Handles NFS-specific concerns: type conversion,
//! async wrapping of sync VFS ops, and lazy write handle management.

use std::collections::{HashMap, VecDeque};
#[cfg(unix)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use nfs3_server::tcp::{NFSTcp, NFSTcpListener};
use nfs3_server::vfs::{
    DirEntryPlus, FileHandleU64, NextResult, NfsFileSystem, NfsReadFileSystem, ReadDirPlusIterator,
};
use nfs3_types::nfs3::{
    Nfs3Option, createverf3, fattr3, filename3, ftype3, nfspath3, nfsstat3, nfstime3, sattr3,
    specdata3,
};
use nfs3_types::xdr_codec::Opaque;

use crate::vfs_ops::{FileAttr, FileKind, VfsOps};

/// LRU cache of open write handles, keyed by inode.
///
/// NFS is stateless but `VfsOps` needs `open_write` / `write` / `release_write`,
/// so we lazily open a handle on first write and reuse it for subsequent
/// writes to the same inode. When the cache exceeds capacity we evict the
/// **least recently used** entry — not an arbitrary one — so a sustained
/// write to a single file does not get its handle thrown out by an unrelated
/// touch.
struct WriteHandleCache {
    /// inode → file handle
    map: HashMap<u64, u64>,
    /// Insertion / touch order. The front is the least recently used.
    order: VecDeque<u64>,
}

impl WriteHandleCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Get the handle for `ino`, marking it as most recently used.
    fn get(&mut self, ino: u64) -> Option<u64> {
        let fh = *self.map.get(&ino)?;
        // Move ino to the back (most recently used). O(N) for N <= 64 is fine;
        // swap to a real LRU crate if MAX_WRITE_HANDLES grows.
        if let Some(pos) = self.order.iter().position(|&i| i == ino) {
            self.order.remove(pos);
        }
        self.order.push_back(ino);
        Some(fh)
    }

    fn insert(&mut self, ino: u64, fh: u64) {
        self.map.insert(ino, fh);
        self.order.push_back(ino);
    }

    /// Evict and return the least recently used `(ino, fh)` pair.
    fn pop_lru(&mut self) -> Option<(u64, u64)> {
        let ino = self.order.pop_front()?;
        let fh = self.map.remove(&ino)?;
        Some((ino, fh))
    }

    fn len(&self) -> usize {
        self.map.len()
    }
}

/// NFS transport adapter, generic over any `VfsOps` implementation.
pub struct NfsAdapter<T: VfsOps> {
    vfs: Arc<T>,
    /// Lazy write handles: see [`WriteHandleCache`].
    write_handles: Mutex<WriteHandleCache>,
}

impl<T: VfsOps> NfsAdapter<T> {
    pub fn new(vfs: T) -> Self {
        Self {
            vfs: Arc::new(vfs),
            write_handles: Mutex::new(WriteHandleCache::new()),
        }
    }

    /// Start the NFS server, binding to `127.0.0.1:{port}`.
    ///
    /// Always binds in writable mode — write operations on read-only VFS
    /// implementations return EROFS naturally via `VfsOps` defaults,
    /// matching how the FUSE adapter handles this.
    /// Pass `port = 0` to let the OS pick a free port.
    pub async fn serve(self, port: u16) -> std::io::Result<NfsServerHandle> {
        let addr = format!("127.0.0.1:{port}");
        let listener = NFSTcpListener::bind(&addr, self).await?;
        let actual_port = listener.get_listen_port();
        // Wrap the server task so unexpected exits (errors or clean returns)
        // are logged instead of silently swallowed.  Panics are caught by
        // tokio at the task boundary and logged at WARN level by default.
        let handle = tokio::spawn(async move {
            match listener.handle_forever().await {
                Ok(()) => {
                    tracing::error!(
                        "NFS server task exited unexpectedly (clean return). \
                         The kernel mount is now stale — file accesses will hang."
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "NFS server task failed: {e}. \
                         The kernel mount is now stale — file accesses will hang."
                    );
                }
            }
        });
        Ok(NfsServerHandle {
            port: actual_port,
            server_task: handle,
        })
    }

    /// Maximum number of concurrent write handles to keep open.
    const MAX_WRITE_HANDLES: usize = 64;

    /// Get or create a write handle for the given inode.
    /// Handles are cached to avoid reopening on consecutive writes to the same file.
    /// When the cache exceeds `MAX_WRITE_HANDLES`, the least recently used
    /// handle is evicted (see [`WriteHandleCache::pop_lru`]).
    fn get_write_handle(&self, ino: u64) -> Result<u64, nfsstat3> {
        let mut cache = self.write_handles.lock().unwrap();
        if let Some(fh) = cache.get(ino) {
            return Ok(fh);
        }
        while cache.len() >= Self::MAX_WRITE_HANDLES {
            if let Some((_, old_fh)) = cache.pop_lru() {
                self.vfs.release_write(old_fh);
            } else {
                break;
            }
        }
        let fh = self.vfs.open_write(ino).map_err(errno_to_nfsstat)?;
        cache.insert(ino, fh);
        Ok(fh)
    }
}

/// Handle to a running NFS server task.
pub struct NfsServerHandle {
    port: u16,
    pub(crate) server_task: tokio::task::JoinHandle<()>,
}

impl NfsServerHandle {
    /// The TCP port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Abort the server task.
    pub fn abort(&self) {
        self.server_task.abort();
    }

    /// Whether the NFS server task is still running.
    ///
    /// Returns `false` if the server exited (error, panic, or unexpected
    /// clean return). A dead server means the kernel mount is stale and file
    /// accesses will hang until the mount is force-unmounted.
    pub fn is_healthy(&self) -> bool {
        !self.server_task.is_finished()
    }
}

/// Handle to a mounted NFS filesystem. Unmounts and stops the server on drop.
pub struct NfsMountHandle {
    // Only read by the macOS/Linux `umount` paths; unused on targets without
    // NFS-mount support (e.g. Windows).
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    pub(crate) mount_point: std::path::PathBuf,
    pub(crate) server_handle: NfsServerHandle,
    /// Whether `do_unmount` has already run successfully. Set by
    /// [`NfsMountHandle::unmount`] so [`Drop`] doesn't repeat the work.
    pub(crate) unmounted: bool,
}

impl NfsMountHandle {
    /// The TCP port the NFS server is listening on.
    pub fn port(&self) -> u16 {
        self.server_handle.port()
    }

    /// Whether the NFS server task is still running.
    ///
    /// Returns `false` if the server exited unexpectedly.  See
    /// [`NfsServerHandle::is_healthy`] for details.
    pub fn is_healthy(&self) -> bool {
        self.server_handle.is_healthy()
    }

    /// Explicitly unmount the NFS mount and stop the server task.
    ///
    /// This is async and uses `tokio::process::Command` so it does not block
    /// the runtime. Prefer this over Drop when you need to surface unmount
    /// failures (e.g. sidecar shutdown). Drop keeps a sync fallback that
    /// uses `std::process::Command` (blocking but bounded) and logs failures
    /// instead of returning them.
    pub async fn unmount(mut self) -> anyhow::Result<()> {
        if self.unmounted {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            let mnt = self.mount_point.display().to_string();
            let status = tokio::process::Command::new("umount")
                .args(["-f", &mnt])
                .status()
                .await?;
            if !status.success() {
                self.server_handle.abort();
                self.unmounted = true;
                anyhow::bail!("umount -f {mnt} failed (exit {status})");
            }
        }
        #[cfg(target_os = "linux")]
        {
            let mnt = self.mount_point.display().to_string();
            let status = tokio::process::Command::new("sudo")
                .args(["umount", "-f", &mnt])
                .status()
                .await?;
            if !status.success() {
                self.server_handle.abort();
                self.unmounted = true;
                anyhow::bail!("sudo umount -f {mnt} failed (exit {status})");
            }
        }
        // NFS mount is not supported on Windows — use ProjFS instead.
        self.server_handle.abort();
        self.unmounted = true;
        Ok(())
    }

    /// Sync best-effort unmount for the Drop impl. Uses blocking
    /// `std::process::Command` — acceptable because Drop is typically
    /// called at shutdown, not on a hot path.
    fn do_unmount_sync(&mut self) {
        if self.unmounted {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let mnt = self.mount_point.display().to_string();
            let _ = std::process::Command::new("umount")
                .args(["-f", &mnt])
                .status();
        }
        #[cfg(target_os = "linux")]
        {
            let mnt = self.mount_point.display().to_string();
            let _ = std::process::Command::new("sudo")
                .args(["umount", "-f", &mnt])
                .status();
        }
        self.server_handle.abort();
        self.unmounted = true;
    }
}

impl Drop for NfsMountHandle {
    fn drop(&mut self) {
        self.do_unmount_sync();
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn file_attr_to_fattr3(attr: &FileAttr) -> fattr3 {
    fattr3 {
        type_: match attr.kind {
            FileKind::RegularFile => ftype3::NF3REG,
            FileKind::Directory => ftype3::NF3DIR,
            FileKind::Symlink => ftype3::NF3LNK,
        },
        mode: u32::from(attr.perm),
        nlink: attr.nlink,
        uid: attr.uid,
        gid: attr.gid,
        size: attr.size,
        used: attr.blocks * 512,
        rdev: specdata3::default(),
        fsid: 0,
        fileid: attr.ino,
        atime: system_time_to_nfstime(attr.atime),
        mtime: system_time_to_nfstime(attr.mtime),
        ctime: system_time_to_nfstime(attr.ctime),
    }
}

fn system_time_to_nfstime(t: SystemTime) -> nfstime3 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => nfstime3 {
            seconds: d.as_secs() as u32,
            nseconds: d.subsec_nanos(),
        },
        Err(_) => nfstime3 {
            seconds: 0,
            nseconds: 0,
        },
    }
}

/// Convert NFS wire-format filename bytes to an OS string.
/// On Unix, filenames are arbitrary byte sequences. On Windows, they must be
/// valid UTF-8 (which conda paths always are — they're sourced from JSON).
fn os_str_from_nfs_bytes(bytes: &[u8]) -> Result<OsString, nfsstat3> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(OsStr::from_bytes(bytes).to_owned())
    }
    #[cfg(not(unix))]
    {
        match std::str::from_utf8(bytes) {
            Ok(s) => Ok(OsString::from(s)),
            Err(_) => Err(nfsstat3::NFS3ERR_INVAL),
        }
    }
}

fn errno_to_nfsstat(errno: i32) -> nfsstat3 {
    match errno {
        libc::ENOENT => nfsstat3::NFS3ERR_NOENT,
        libc::EACCES => nfsstat3::NFS3ERR_ACCES,
        libc::ENOTDIR => nfsstat3::NFS3ERR_NOTDIR,
        libc::EISDIR => nfsstat3::NFS3ERR_ISDIR,
        libc::EROFS => nfsstat3::NFS3ERR_ROFS,
        libc::EEXIST => nfsstat3::NFS3ERR_EXIST,
        libc::ENOTEMPTY => nfsstat3::NFS3ERR_NOTEMPTY,
        libc::ENOSPC => nfsstat3::NFS3ERR_NOSPC,
        _ => nfsstat3::NFS3ERR_IO,
    }
}

// ---------------------------------------------------------------------------
// ReadDirPlus iterator backed by a pre-fetched Vec
// ---------------------------------------------------------------------------

struct VfsDirPlusIter {
    entries: Vec<DirEntryPlus<FileHandleU64>>,
    pos: usize,
}

impl ReadDirPlusIterator<FileHandleU64> for VfsDirPlusIter {
    async fn next(&mut self) -> NextResult<DirEntryPlus<FileHandleU64>> {
        if self.pos >= self.entries.len() {
            return NextResult::Eof;
        }
        let entry = self.entries[self.pos].clone();
        self.pos += 1;
        NextResult::Ok(entry)
    }
}

// ---------------------------------------------------------------------------
// NfsReadFileSystem implementation
// ---------------------------------------------------------------------------

impl<T: VfsOps> NfsReadFileSystem for NfsAdapter<T> {
    type Handle = FileHandleU64;

    fn root_dir(&self) -> FileHandleU64 {
        FileHandleU64::new(1)
    }

    async fn lookup(
        &self,
        dirid: &FileHandleU64,
        filename: &filename3<'_>,
    ) -> Result<FileHandleU64, nfsstat3> {
        let vfs = self.vfs.clone();
        let parent_ino: u64 = dirid.as_u64();
        let name_bytes = filename.0.as_ref().to_vec();
        tokio::task::spawn_blocking(move || {
            let name = &os_str_from_nfs_bytes(&name_bytes)?;
            let attr = vfs.lookup(parent_ino, name).map_err(errno_to_nfsstat)?;
            Ok(FileHandleU64::new(attr.ino))
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn getattr(&self, id: &FileHandleU64) -> Result<fattr3, nfsstat3> {
        let vfs = self.vfs.clone();
        let ino = id.as_u64();
        tokio::task::spawn_blocking(move || {
            let attr = vfs.getattr(ino).map_err(errno_to_nfsstat)?;
            Ok(file_attr_to_fattr3(&attr))
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn read(
        &self,
        id: &FileHandleU64,
        offset: u64,
        count: u32,
    ) -> Result<(Vec<u8>, bool), nfsstat3> {
        let vfs = self.vfs.clone();
        let ino = id.as_u64();
        tokio::task::spawn_blocking(move || {
            let data = vfs.read(ino, offset, count).map_err(errno_to_nfsstat)?;
            let eof = (data.len() as u32) < count;
            Ok((data, eof))
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn readdirplus(
        &self,
        dirid: &FileHandleU64,
        cookie: u64,
    ) -> Result<impl ReadDirPlusIterator<FileHandleU64>, nfsstat3> {
        let vfs = self.vfs.clone();
        let ino = dirid.as_u64();
        tokio::task::spawn_blocking(move || {
            let dir_entries = vfs.readdir(ino, cookie).map_err(errno_to_nfsstat)?;
            let mut entries = Vec::with_capacity(dir_entries.len());
            for (i, de) in dir_entries.into_iter().enumerate() {
                let attr = vfs.getattr(de.ino).ok().map(|a| file_attr_to_fattr3(&a));
                entries.push(DirEntryPlus {
                    fileid: de.ino,
                    name: filename3(Opaque::owned(de.name.as_encoded_bytes().to_vec())),
                    cookie: cookie + i as u64 + 1,
                    name_attributes: attr,
                    name_handle: Some(FileHandleU64::new(de.ino)),
                });
            }
            Ok(VfsDirPlusIter { entries, pos: 0 })
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn readlink(&self, id: &FileHandleU64) -> Result<nfspath3<'_>, nfsstat3> {
        let vfs = self.vfs.clone();
        let ino = id.as_u64();
        tokio::task::spawn_blocking(move || {
            let target = vfs.readlink(ino).map_err(errno_to_nfsstat)?;
            let bytes = target.as_os_str().as_encoded_bytes().to_vec();
            Ok(nfspath3(Opaque::owned(bytes)))
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }
}

// ---------------------------------------------------------------------------
// NfsFileSystem (writable) implementation
// ---------------------------------------------------------------------------

impl<T: VfsOps> NfsFileSystem for NfsAdapter<T> {
    async fn setattr(&self, id: &FileHandleU64, setattr: sattr3) -> Result<fattr3, nfsstat3> {
        let vfs = self.vfs.clone();
        let ino = id.as_u64();
        tokio::task::spawn_blocking(move || {
            let size = match setattr.size {
                Nfs3Option::Some(s) => Some(s),
                Nfs3Option::None => None,
            };
            let mode = match setattr.mode {
                Nfs3Option::Some(m) => Some(m),
                Nfs3Option::None => None,
            };
            let attr = vfs.setattr(ino, size, mode).map_err(errno_to_nfsstat)?;
            Ok(file_attr_to_fattr3(&attr))
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn write(
        &self,
        id: &FileHandleU64,
        offset: u64,
        data: &[u8],
    ) -> Result<fattr3, nfsstat3> {
        let ino = id.as_u64();
        let data = data.to_vec();

        // Retry once on EIO: a concurrent write to a different file can evict
        // (and close) our cached write handle between acquiring it and using it,
        // since the LRU cache is bounded. Eviction also drops the cache entry,
        // so re-acquiring the handle reopens a fresh one.
        for attempt in 0..2 {
            let fh = self.get_write_handle(ino)?;
            let vfs = self.vfs.clone();
            let data = data.clone();
            let write_result = tokio::task::spawn_blocking(move || vfs.write(fh, offset, &data))
                .await
                .unwrap_or_else(|e| {
                    tracing::error!("NFS write handler panicked: {e}");
                    Err(libc::EIO)
                });

            match write_result {
                Ok(_) => {
                    let vfs = self.vfs.clone();
                    return tokio::task::spawn_blocking(move || vfs.getattr(ino))
                        .await
                        .unwrap_or_else(|e| {
                            tracing::error!("NFS handler panicked: {e}");
                            Err(libc::EIO)
                        })
                        .map(|attr| file_attr_to_fattr3(&attr))
                        .map_err(errno_to_nfsstat);
                }
                // Evicted handle on the first try: fall through to reopen+retry.
                Err(libc::EIO) if attempt == 0 => {}
                Err(e) => return Err(errno_to_nfsstat(e)),
            }
        }
        Err(nfsstat3::NFS3ERR_IO)
    }

    async fn create(
        &self,
        dirid: &FileHandleU64,
        filename: &filename3<'_>,
        attr: sattr3,
    ) -> Result<(FileHandleU64, fattr3), nfsstat3> {
        let vfs = self.vfs.clone();
        let parent_ino = dirid.as_u64();
        let name_bytes = filename.0.as_ref().to_vec();
        let mode = match attr.mode {
            Nfs3Option::Some(m) => m,
            Nfs3Option::None => 0o644,
        };
        tokio::task::spawn_blocking(move || {
            let name = &os_str_from_nfs_bytes(&name_bytes)?;
            let (file_attr, fh) = vfs
                .create(parent_ino, name, mode)
                .map_err(errno_to_nfsstat)?;
            vfs.release_write(fh);
            Ok((
                FileHandleU64::new(file_attr.ino),
                file_attr_to_fattr3(&file_attr),
            ))
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn create_exclusive(
        &self,
        dirid: &FileHandleU64,
        filename: &filename3<'_>,
        _createverf: createverf3,
    ) -> Result<FileHandleU64, nfsstat3> {
        // Use regular create — exclusive semantics aren't critical for our use case
        let vfs = self.vfs.clone();
        let parent_ino = dirid.as_u64();
        let name_bytes = filename.0.as_ref().to_vec();
        tokio::task::spawn_blocking(move || {
            let name = &os_str_from_nfs_bytes(&name_bytes)?;
            let (file_attr, fh) = vfs.create(parent_ino, name, 0o644).map_err(|errno| {
                tracing::warn!(
                    "create_exclusive failed: parent={parent_ino} name={name:?} errno={errno}"
                );
                errno_to_nfsstat(errno)
            })?;
            vfs.release_write(fh);
            Ok(FileHandleU64::new(file_attr.ino))
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("create_exclusive spawn_blocking panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn mkdir(
        &self,
        dirid: &FileHandleU64,
        dirname: &filename3<'_>,
    ) -> Result<(FileHandleU64, fattr3), nfsstat3> {
        let vfs = self.vfs.clone();
        let parent_ino = dirid.as_u64();
        let name_bytes = dirname.0.as_ref().to_vec();
        tokio::task::spawn_blocking(move || {
            let name = &os_str_from_nfs_bytes(&name_bytes)?;
            let dir_attr = vfs
                .mkdir(parent_ino, name, 0o755)
                .map_err(errno_to_nfsstat)?;
            Ok((
                FileHandleU64::new(dir_attr.ino),
                file_attr_to_fattr3(&dir_attr),
            ))
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn remove(
        &self,
        dirid: &FileHandleU64,
        filename: &filename3<'_>,
    ) -> Result<(), nfsstat3> {
        let vfs = self.vfs.clone();
        let parent_ino = dirid.as_u64();
        let name_bytes = filename.0.as_ref().to_vec();
        tokio::task::spawn_blocking(move || {
            let name = &os_str_from_nfs_bytes(&name_bytes)?;
            vfs.unlink(parent_ino, name).map_err(errno_to_nfsstat)
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn rename<'a>(
        &self,
        from_dirid: &FileHandleU64,
        from_filename: &filename3<'a>,
        to_dirid: &FileHandleU64,
        to_filename: &filename3<'a>,
    ) -> Result<(), nfsstat3> {
        let vfs = self.vfs.clone();
        let from_parent = from_dirid.as_u64();
        let to_parent = to_dirid.as_u64();
        let from_name = from_filename.0.as_ref().to_vec();
        let to_name = to_filename.0.as_ref().to_vec();
        tokio::task::spawn_blocking(move || {
            let from = os_str_from_nfs_bytes(&from_name)?;
            let to = os_str_from_nfs_bytes(&to_name)?;
            vfs.rename(from_parent, &from, to_parent, &to, 0)
                .map_err(errno_to_nfsstat)
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("NFS handler panicked: {e}");
            Err(nfsstat3::NFS3ERR_IO)
        })
    }

    async fn symlink<'a>(
        &self,
        _dirid: &FileHandleU64,
        _linkname: &filename3<'a>,
        _symlink: &nfspath3<'a>,
        _attr: &sattr3,
    ) -> Result<(FileHandleU64, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_NOTSUPP)
    }
}
