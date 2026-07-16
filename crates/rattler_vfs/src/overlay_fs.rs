//! Writable overlay filesystem.
//!
//! Wraps a read-only `VfsOps` (the lower layer) with a persistent upper
//! directory for copy-on-write semantics. Implements the same `VfsOps` trait
//! so it can be used interchangeably with the read-only VFS.

use libc::{EIO, ENOENT};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

mod inode;

use crate::overlay::{OverlayState, STATE_FILENAME, STATE_LOCK_FILENAME, STATE_TMP_FILENAME};
use crate::vfs_ops::{ContentSource, DirEntry, FileAttr, FileKind, VfsOps, set_file_permissions};
use inode::{ResolvedIno, UPPER_INODE_BASE, UpperInodeMap};

/// A writable overlay filesystem that wraps a read-only lower layer.
pub struct OverlayFS<T: VfsOps> {
    lower: T,
    state: Mutex<OverlayState>,
    /// Immutable overlay directory path — avoids locking state just to build upper paths.
    overlay_dir: PathBuf,
    upper_inodes: Mutex<UpperInodeMap>,
    lower_ino_cache: Mutex<HashMap<PathBuf, u64>>,
    open_files: Mutex<HashMap<u64, File>>,
    next_fh: AtomicU64,
    /// Lower inodes promoted to upper layer via rename or `open_write` COW.
    /// Maps `lower_ino` → `upper_ino` so the kernel's cached handle still works.
    promoted: Mutex<HashMap<u64, u64>>,
}

impl<T: VfsOps> OverlayFS<T> {
    /// Wrap a lower VFS with a pre-loaded overlay state.
    ///
    /// The state must have been loaded via [`OverlayState::load`] with
    /// matching `env_hash` and `transport`; this constructor does not
    /// revalidate. Splitting validation from VFS consumption lets the caller
    /// detect a stale overlay (e.g. version mismatch), wipe it, and retry
    /// without losing the lower VFS — see `create_overlay` in `lib.rs`.
    pub fn wrap(lower: T, state: OverlayState) -> Result<Self, String> {
        let overlay_dir = state.dir().to_path_buf();

        let mut lower_ino_cache = HashMap::new();
        lower_ino_cache.insert(PathBuf::new(), 1u64);

        let overlay = OverlayFS {
            lower,
            state: Mutex::new(state),
            overlay_dir: overlay_dir.clone(),
            upper_inodes: Mutex::new(UpperInodeMap::new()),
            lower_ino_cache: Mutex::new(lower_ino_cache),
            open_files: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            promoted: Mutex::new(HashMap::new()),
        };

        overlay.scan_upper(&overlay_dir)?;
        Ok(overlay)
    }

    /// Convenience: load state and wrap in one call.
    ///
    /// **Consumes `lower` on validation failure.** Production code that needs
    /// transparent recovery from a stale overlay should call
    /// [`OverlayState::load`] first and then [`OverlayFS::wrap`] separately —
    /// see `create_overlay` in `lib.rs`. This helper exists for tests and
    /// simple callers that don't need recovery.
    pub fn new(
        lower: T,
        overlay_dir: PathBuf,
        env_hash: String,
        transport: String,
    ) -> Result<Self, String> {
        let state = OverlayState::load(
            overlay_dir,
            env_hash,
            transport,
            crate::OverlayMismatch::Error,
        )
        .map_err(|e| format!("failed to load overlay state: {e}"))?;
        Self::wrap(lower, state)
    }

    fn scan_upper(&self, base: &Path) -> Result<(), String> {
        let entries = match fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default();

            // Skip overlay internal files
            if name == STATE_FILENAME || name == STATE_TMP_FILENAME || name == STATE_LOCK_FILENAME {
                continue;
            }

            let relative = path
                .strip_prefix(&self.overlay_dir)
                .map_err(|e| format!("path error: {e}"))?
                .to_path_buf();
            self.assign_upper_ino(relative);

            if path.is_dir() {
                self.scan_upper(&path)?;
            }
        }
        Ok(())
    }

    fn assign_upper_ino(&self, virtual_path: PathBuf) -> u64 {
        self.upper_inodes
            .lock()
            .unwrap()
            .get_or_assign(virtual_path)
    }

    /// Get the on-disk upper layer path for a virtual path (no lock needed).
    fn upper_path(&self, virtual_path: &Path) -> PathBuf {
        self.overlay_dir.join(virtual_path)
    }

    fn resolve_path(&self, parent: u64, name: &OsStr) -> Result<PathBuf, i32> {
        let parent_path = match self.resolve_ino(parent)? {
            ResolvedIno::Upper(p) | ResolvedIno::Lower(_, p) => p,
        };
        Ok(parent_path.join(name))
    }

    fn resolve_ino(&self, ino: u64) -> Result<ResolvedIno, i32> {
        // Check if this lower inode was promoted to upper via rename or COW
        let effective = if ino < UPPER_INODE_BASE {
            self.promoted.lock().unwrap().get(&ino).copied()
        } else {
            None
        };
        let effective_ino = effective.unwrap_or(ino);

        if effective_ino >= UPPER_INODE_BASE {
            let map = self.upper_inodes.lock().unwrap();
            let path = map.path_for(effective_ino).cloned().ok_or_else(|| {
                tracing::warn!(
                    "overlay resolve_ino: upper ino={} not found in inode map",
                    effective_ino
                );
                ENOENT
            })?;
            Ok(ResolvedIno::Upper(path))
        } else {
            let path = self.lower.ino_to_path(effective_ino)?;
            Ok(ResolvedIno::Lower(effective_ino, path))
        }
    }

    fn lower_ino_for_path(&self, path: &Path) -> Option<u64> {
        if path.as_os_str().is_empty() {
            return Some(1);
        }

        // Check cache for the full path first
        {
            let cache = self.lower_ino_cache.lock().unwrap();
            if let Some(&ino) = cache.get(path) {
                return Some(ino);
            }
        }

        // Walk from root, batch cache updates
        let mut current_ino = 1u64;
        let mut current_path = PathBuf::new();
        let mut new_entries = Vec::new();

        for component in path.components() {
            let name = component.as_os_str();

            // Check cache for intermediate prefix
            current_path.push(name);
            {
                let cache = self.lower_ino_cache.lock().unwrap();
                if let Some(&cached_ino) = cache.get(&current_path) {
                    current_ino = cached_ino;
                    continue;
                }
            }

            match self.lower.lookup(current_ino, name) {
                Ok(attr) => {
                    current_ino = attr.ino;
                    new_entries.push((current_path.clone(), current_ino));
                }
                Err(_) => return None,
            }
        }

        // Batch insert all new cache entries
        if !new_entries.is_empty() {
            let mut cache = self.lower_ino_cache.lock().unwrap();
            for (path, ino) in new_entries {
                cache.insert(path, ino);
            }
        }

        Some(current_ino)
    }

    fn make_upper_attr(&self, path: &Path, ino: u64) -> Result<FileAttr, i32> {
        let full_path = self.upper_path(path);
        let metadata = fs::symlink_metadata(&full_path).map_err(|_e| ENOENT)?;
        Ok(FileAttr::from_metadata(&metadata, ino))
    }

    fn ensure_upper_parent(&self, virtual_path: &Path) -> Result<(), i32> {
        if let Some(parent) = virtual_path.parent() {
            fs::create_dir_all(self.upper_path(parent)).map_err(|e| {
                tracing::warn!(
                    "failed to create upper parent for {:?}: {}",
                    virtual_path,
                    e
                );
                EIO
            })?;
        }
        Ok(())
    }

    fn copy_to_upper(&self, virtual_path: &Path, lower_ino: u64) -> Result<PathBuf, i32> {
        self.ensure_upper_parent(virtual_path)?;
        let upper_path = self.upper_path(virtual_path);

        if upper_path.exists() {
            return Ok(upper_path);
        }

        // Check if this is a directory — directories need recursive COW
        if let Ok(attr) = self.lower.getattr(lower_ino)
            && attr.kind == FileKind::Directory
        {
            return self.copy_dir_to_upper(virtual_path, lower_ino);
        }

        if let Ok(ContentSource::Direct(source)) = self.lower.content_source(lower_ino) {
            reflink_copy::reflink_or_copy(&source, &upper_path).map_err(|e| {
                tracing::warn!("COW copy failed {:?} -> {:?}: {}", source, upper_path, e);
                EIO
            })?;
        } else {
            // Transformed or Virtual — read all content via the VFS
            let data = self.lower.read(lower_ino, 0, u32::MAX)?;
            fs::write(&upper_path, &data).map_err(|e| {
                tracing::warn!("COW write failed {:?}: {}", upper_path, e);
                EIO
            })?;
        }
        Ok(upper_path)
    }

    /// Recursively copy a lower-layer directory and all its visible children
    /// to the upper layer. Used when renaming a directory from lower to upper.
    fn copy_dir_to_upper(&self, virtual_path: &Path, lower_ino: u64) -> Result<PathBuf, i32> {
        let upper_path = self.upper_path(virtual_path);
        fs::create_dir_all(&upper_path).map_err(|e| {
            tracing::warn!("COW mkdir failed {:?}: {}", upper_path, e);
            EIO
        })?;

        let state = self.state.lock().unwrap();
        let entries = self.lower.readdir(lower_ino, 0).unwrap_or_default();
        // Collect entries to avoid holding state lock during recursive COW
        let children: Vec<_> = entries
            .into_iter()
            .filter(|e| {
                e.name != "." && e.name != ".." && !state.is_whiteout(&virtual_path.join(&e.name))
            })
            .collect();
        drop(state);

        for child in children {
            let child_path = virtual_path.join(&child.name);
            self.copy_to_upper(&child_path, child.ino)?;
        }

        Ok(upper_path)
    }
}

impl<T: VfsOps> VfsOps for OverlayFS<T> {
    fn lookup(&self, parent: u64, name: &OsStr) -> Result<FileAttr, i32> {
        let parent_path = match self.resolve_ino(parent)? {
            ResolvedIno::Upper(p) | ResolvedIno::Lower(_, p) => p,
        };
        let virtual_path = parent_path.join(name);

        // Single state lock: check whiteout and opaque in one acquisition
        {
            let state = self.state.lock().unwrap();
            if state.is_whiteout(&virtual_path) {
                return Err(ENOENT);
            }
            if state.is_opaque(&parent_path) {
                return Err(ENOENT);
            }
        }

        // Check upper layer — use symlink_metadata directly (avoids TOCTOU of exists + stat)
        let upper = self.upper_path(&virtual_path);
        if let Ok(metadata) = fs::symlink_metadata(&upper) {
            let ino = self.assign_upper_ino(virtual_path.clone());
            return Ok(FileAttr::from_metadata(&metadata, ino));
        }

        // Fall through to lower
        if let Some(lower_parent_ino) = self.lower_ino_for_path(&parent_path) {
            let result = self.lower.lookup(lower_parent_ino, name);
            if let Ok(ref attr) = result {
                self.lower_ino_cache
                    .lock()
                    .unwrap()
                    .insert(virtual_path, attr.ino);
            }
            result
        } else {
            Err(ENOENT)
        }
    }

    fn getattr(&self, ino: u64) -> Result<FileAttr, i32> {
        match self.resolve_ino(ino)? {
            ResolvedIno::Upper(path) => {
                // Try upper first, fall through to lower if it's just a structural dir
                match self.make_upper_attr(&path, ino) {
                    Ok(attr) => Ok(attr),
                    Err(_) => {
                        if let Some(lower_ino) = self.lower_ino_for_path(&path) {
                            self.lower.getattr(lower_ino)
                        } else {
                            Err(ENOENT)
                        }
                    }
                }
            }
            ResolvedIno::Lower(lower_ino, _) => self.lower.getattr(lower_ino),
        }
    }

    fn readlink(&self, ino: u64) -> Result<PathBuf, i32> {
        match self.resolve_ino(ino)? {
            ResolvedIno::Upper(path) => {
                let full = self.upper_path(&path);
                fs::read_link(&full).map_err(|_e| EIO)
            }
            ResolvedIno::Lower(lower_ino, _) => self.lower.readlink(lower_ino),
        }
    }

    fn read(&self, ino: u64, offset: u64, size: u32) -> Result<Vec<u8>, i32> {
        match self.resolve_ino(ino)? {
            ResolvedIno::Upper(path) => {
                let full = self.upper_path(&path);
                if full.exists() && !full.is_dir() {
                    let mut file = File::open(&full).map_err(|_e| EIO)?;
                    file.seek(SeekFrom::Start(offset)).map_err(|_e| EIO)?;
                    let mut buf = vec![0u8; size as usize];
                    let n = file.read(&mut buf).map_err(|_e| EIO)?;
                    buf.truncate(n);
                    return Ok(buf);
                }
                // Fall through to lower
                if let Some(lower_ino) = self.lower_ino_for_path(&path) {
                    self.lower.read(lower_ino, offset, size)
                } else {
                    Err(ENOENT)
                }
            }
            ResolvedIno::Lower(lower_ino, _) => self.lower.read(lower_ino, offset, size),
        }
    }

    fn content_source(&self, ino: u64) -> Result<ContentSource, i32> {
        match self.resolve_ino(ino)? {
            ResolvedIno::Upper(path) => {
                let full = self.upper_path(&path);
                if full.exists() && !full.is_dir() {
                    Ok(ContentSource::Direct(full))
                } else if let Some(lower_ino) = self.lower_ino_for_path(&path) {
                    self.lower.content_source(lower_ino)
                } else {
                    Err(ENOENT)
                }
            }
            ResolvedIno::Lower(lower_ino, _) => self.lower.content_source(lower_ino),
        }
    }

    fn open_write(&self, ino: u64) -> Result<u64, i32> {
        let (virtual_path, needs_cow_from) = match self.resolve_ino(ino)? {
            ResolvedIno::Upper(path) => {
                let p = self.upper_path(&path);
                if p.exists() {
                    (path, None)
                } else {
                    let li = self.lower_ino_for_path(&path).ok_or(ENOENT)?;
                    (path, Some(li))
                }
            }
            ResolvedIno::Lower(li, path) => (path, Some(li)),
        };

        if let Some(li) = needs_cow_from {
            self.copy_to_upper(&virtual_path, li)?;
            // Promote: the kernel may call getattr/read with the original lower
            // inode after this COW, so redirect it to the upper copy.
            if ino < UPPER_INODE_BASE {
                let upper_ino = self.assign_upper_ino(virtual_path.clone());
                self.promoted.lock().unwrap().insert(ino, upper_ino);
            }
        }

        let upper_path = self.upper_path(&virtual_path);
        let file = File::options()
            .read(true)
            .write(true)
            .open(&upper_path)
            .map_err(|e| {
                tracing::warn!("overlay open write failed {:?}: {}", upper_path, e);
                EIO
            })?;
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        self.open_files.lock().unwrap().insert(fh, file);
        Ok(fh)
    }

    fn read_handle(&self, fh: u64, offset: u64, size: u32) -> Result<Vec<u8>, i32> {
        let mut files = self.open_files.lock().unwrap();
        let file = files.get_mut(&fh).ok_or(EIO)?;
        file.seek(SeekFrom::Start(offset)).map_err(|_e| EIO)?;
        let mut buf = vec![0u8; size as usize];
        let n = file.read(&mut buf).map_err(|_e| EIO)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn release_write(&self, fh: u64) {
        self.open_files.lock().unwrap().remove(&fh);
    }

    fn readdir(&self, ino: u64, offset: u64) -> Result<Vec<DirEntry>, i32> {
        let (dir_path, lower_dir_ino) = match self.resolve_ino(ino)? {
            ResolvedIno::Upper(p) => {
                let li = self.lower_ino_for_path(&p);
                (p, li)
            }
            ResolvedIno::Lower(li, p) => (p, Some(li)),
        };
        let state = self.state.lock().unwrap();

        // Collect entries by name — upper overrides lower
        let mut entries_by_name: HashMap<OsString, DirEntry> = HashMap::new();

        // Start with lower layer entries — unless this directory is opaque
        let lower_ino = if state.is_opaque(&dir_path) {
            None
        } else {
            lower_dir_ino
        };
        if let Some(lower_ino) = lower_ino
            && let Ok(lower_entries) = self.lower.readdir(lower_ino, 0)
        {
            for entry in lower_entries {
                // Skip . and .. — we'll add them ourselves
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                let child_path = dir_path.join(&entry.name);
                if !state.is_whiteout(&child_path) {
                    entries_by_name.insert(entry.name.clone(), entry);
                }
            }
        }

        // Overlay upper layer entries
        let upper_dir = self.upper_path(&dir_path);
        drop(state);

        if let Ok(read_dir) = fs::read_dir(&upper_dir) {
            for entry in read_dir.flatten() {
                let name = entry.file_name();
                if name == STATE_FILENAME
                    || name == STATE_LOCK_FILENAME
                    || name.to_str().is_some_and(|s| s.ends_with(".tmp"))
                {
                    continue;
                }
                let child_path = dir_path.join(&name);
                let child_ino = self.assign_upper_ino(child_path);
                let kind = if entry.path().is_dir() {
                    FileKind::Directory
                } else {
                    FileKind::RegularFile
                };
                entries_by_name.insert(
                    name.clone(),
                    DirEntry {
                        ino: child_ino,
                        kind,
                        name,
                    },
                );
            }
        }

        // Build final list with . and .. at the front
        let mut result = Vec::new();
        if offset == 0 {
            result.push(DirEntry {
                ino: 1, // parent — simplified
                kind: FileKind::Directory,
                name: OsString::from(".."),
            });
        }
        if offset <= 1 {
            result.push(DirEntry {
                ino,
                kind: FileKind::Directory,
                name: OsString::from("."),
            });
        }

        // Sort entries by name for deterministic ordering across readdir calls.
        // HashMap iteration order is non-deterministic — without sorting, offset-based
        // pagination would skip different entries on each call.
        let mut sorted_entries: Vec<_> = entries_by_name.into_values().collect();
        sorted_entries.sort_by(|a, b| a.name.cmp(&b.name));

        let skip = offset.saturating_sub(2) as usize;
        result.extend(sorted_entries.into_iter().skip(skip));
        Ok(result)
    }

    fn create(&self, parent: u64, name: &OsStr, mode: u32) -> Result<(FileAttr, u64), i32> {
        let virtual_path = self.resolve_path(parent, name).inspect_err(|&e| {
            tracing::warn!(
                "overlay create: resolve_path failed for parent={} name={:?}: errno={}",
                parent,
                name,
                e
            );
        })?;
        self.ensure_upper_parent(&virtual_path).inspect_err(|&e| {
            tracing::warn!(
                "overlay create: ensure_upper_parent failed for {:?}: errno={}",
                virtual_path,
                e
            );
        })?;

        let upper_path = self.upper_path(&virtual_path);
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&upper_path)
            .map_err(|e| {
                tracing::warn!("create failed {:?}: {}", upper_path, e);
                EIO
            })?;
        set_file_permissions(&upper_path, mode).ok();

        self.state
            .lock()
            .unwrap()
            .remove_whiteout(&virtual_path)
            .map_err(|e| {
                tracing::warn!(
                    "overlay create: remove_whiteout/flush failed for {:?}: {}",
                    virtual_path,
                    e
                );
                EIO
            })?;

        let ino = self.assign_upper_ino(virtual_path.clone());
        let attr = self.make_upper_attr(&virtual_path, ino)?;

        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        self.open_files.lock().unwrap().insert(fh, file);

        Ok((attr, fh))
    }

    fn write(&self, fh: u64, offset: u64, data: &[u8]) -> Result<u32, i32> {
        let mut files = self.open_files.lock().unwrap();
        let file = files.get_mut(&fh).ok_or_else(|| {
            tracing::warn!("overlay write: fh={} not found in open_files", fh);
            EIO
        })?;
        file.seek(SeekFrom::Start(offset)).map_err(|e| {
            tracing::warn!(
                "overlay write: seek failed fh={} offset={}: {}",
                fh,
                offset,
                e
            );
            EIO
        })?;
        file.write_all(data).map_err(|e| {
            tracing::warn!("overlay write: write failed fh={}: {}", fh, e);
            EIO
        })?;
        Ok(data.len() as u32)
    }

    fn unlink(&self, parent: u64, name: &OsStr) -> Result<(), i32> {
        let virtual_path = self.resolve_path(parent, name)?;
        let upper_path = self.upper_path(&virtual_path);

        // If this is a lower-layer directory with visible children, reject.
        // The NFS server routes both REMOVE and RMDIR through unlink, so we
        // must enforce ENOTEMPTY here as well as in rmdir.
        if let Some(lower_ino) = self.lower_ino_for_path(&virtual_path)
            && let Ok(attr) = self.lower.getattr(lower_ino)
            && attr.kind == FileKind::Directory
        {
            let state = self.state.lock().unwrap();
            if let Ok(lower_entries) = self.lower.readdir(lower_ino, 0) {
                let has_visible = lower_entries.iter().any(|e| {
                    if e.name == "." || e.name == ".." {
                        return false;
                    }
                    !state.is_whiteout(&virtual_path.join(&e.name))
                });
                if has_visible {
                    return Err(libc::ENOTEMPTY);
                }
            }
        }

        // Remove from upper if present (ignore NotFound).
        // The NFS server routes both REMOVE and RMDIR through this function,
        // so handle both files and directories. Try remove_file first; on
        // failure (EPERM on macOS, EISDIR on Linux) fall back to remove_dir.
        if let Err(e) = fs::remove_file(&upper_path)
            && e.kind() != std::io::ErrorKind::NotFound
            && let Err(e2) = fs::remove_dir(&upper_path)
            && e2.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "unlink/rmdir failed {:?}: file={}, dir={}",
                upper_path,
                e,
                e2
            );
            return Err(e2.raw_os_error().unwrap_or(EIO));
        }

        // Only whiteout if the file exists in the lower layer — no point tracking
        // deletions of upper-only files (e.g. .pyc temp files)
        if self.lower_ino_for_path(&virtual_path).is_some() {
            self.state
                .lock()
                .unwrap()
                .add_whiteout(virtual_path)
                .map_err(|_e| EIO)?;
        }

        Ok(())
    }

    fn mkdir(&self, parent: u64, name: &OsStr, mode: u32) -> Result<FileAttr, i32> {
        let virtual_path = self.resolve_path(parent, name)?;
        let upper_path = self.upper_path(&virtual_path);

        fs::create_dir_all(&upper_path).map_err(|e| {
            tracing::warn!("mkdir failed {:?}: {}", upper_path, e);
            EIO
        })?;
        set_file_permissions(&upper_path, mode).ok();

        // Only mark opaque if previously whiteout'd (rmdir + mkdir pattern)
        let mut state = self.state.lock().unwrap();
        let was_whiteoutd = state.is_whiteout(&virtual_path);
        state.remove_whiteout(&virtual_path).map_err(|_e| EIO)?;
        if was_whiteoutd && self.lower_ino_for_path(&virtual_path).is_some() {
            state
                .add_opaque_dir(virtual_path.clone())
                .map_err(|_e| EIO)?;
        }
        drop(state);

        let ino = self.assign_upper_ino(virtual_path.clone());
        self.make_upper_attr(&virtual_path, ino)
    }

    fn rmdir(&self, parent: u64, name: &OsStr) -> Result<(), i32> {
        let virtual_path = self.resolve_path(parent, name)?;
        let upper_path = self.upper_path(&virtual_path);

        // Check for visible children before removing. A lower-layer directory
        // may still contain files not covered by whiteouts — removing it would
        // hide them all (the whiteout on the directory itself blocks lookups for
        // every child).
        if let Some(lower_ino) = self.lower_ino_for_path(&virtual_path) {
            let state = self.state.lock().unwrap();
            if let Ok(lower_entries) = self.lower.readdir(lower_ino, 0) {
                let has_visible = lower_entries.iter().any(|e| {
                    if e.name == "." || e.name == ".." {
                        return false;
                    }
                    !state.is_whiteout(&virtual_path.join(&e.name))
                });
                if has_visible {
                    return Err(libc::ENOTEMPTY);
                }
            }
        }

        if let Err(e) = fs::remove_dir(&upper_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("rmdir failed {:?}: {}", upper_path, e);
            return Err(EIO);
        }

        if self.lower_ino_for_path(&virtual_path).is_some() {
            self.state
                .lock()
                .unwrap()
                .add_whiteout(virtual_path)
                .map_err(|_e| EIO)?;
        }

        Ok(())
    }

    fn rename(
        &self,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
    ) -> Result<(), i32> {
        // Handle RENAME_NOREPLACE: fail if destination exists
        #[cfg(target_os = "linux")]
        if flags & libc::RENAME_NOREPLACE != 0 {
            let dst_check = self.resolve_path(newparent, newname)?;
            if self.upper_path(&dst_check).exists() {
                return Err(libc::EEXIST);
            }
            let state = self.state.lock().unwrap();
            if !state.is_whiteout(&dst_check) && self.lower_ino_for_path(&dst_check).is_some() {
                return Err(libc::EEXIST);
            }
        }
        // Suppress unused warning on non-Linux
        #[cfg(not(target_os = "linux"))]
        let _ = flags;

        let src_path = self.resolve_path(parent, name)?;
        let dst_path = self.resolve_path(newparent, newname)?;
        let upper_src = self.upper_path(&src_path);
        let upper_dst = self.upper_path(&dst_path);

        self.ensure_upper_parent(&dst_path)?;

        let upper_src_existed = upper_src.exists();
        if upper_src_existed {
            // If this is a directory that also has lower-layer content, COW
            // any lower children not already present in upper before renaming.
            // Without this, the lower children are lost (whiteout hides them,
            // and the destination only gets the upper content).
            if let Some(lower_ino) = self.lower_ino_for_path(&src_path)
                && let Ok(attr) = self.lower.getattr(lower_ino)
                && attr.kind == FileKind::Directory
            {
                let state = self.state.lock().unwrap();
                let entries = self.lower.readdir(lower_ino, 0).unwrap_or_default();
                let children: Vec<_> = entries
                    .into_iter()
                    .filter(|e| {
                        e.name != "."
                            && e.name != ".."
                            && !state.is_whiteout(&src_path.join(&e.name))
                    })
                    .collect();
                drop(state);
                for child in children {
                    let child_path = src_path.join(&child.name);
                    // copy_to_upper is idempotent — skips if already in upper
                    self.copy_to_upper(&child_path, child.ino)?;
                }
            }
            fs::rename(&upper_src, &upper_dst).map_err(|e| {
                tracing::warn!("rename failed {:?} -> {:?}: {}", upper_src, upper_dst, e);
                EIO
            })?;
        } else {
            // Source is in lower — COW then move
            let parent_path = match self.resolve_ino(parent)? {
                ResolvedIno::Upper(p) | ResolvedIno::Lower(_, p) => p,
            };
            let lower_parent_ino = self.lower_ino_for_path(&parent_path).ok_or(ENOENT)?;
            let attr = self.lower.lookup(lower_parent_ino, name)?;
            self.copy_to_upper(&src_path, attr.ino)?;
            fs::rename(&upper_src, &upper_dst).map_err(|e| {
                tracing::warn!(
                    "rename after COW failed {:?} -> {:?}: {}",
                    upper_src,
                    upper_dst,
                    e
                );
                EIO
            })?;

            // Promote: the kernel holds the lower inode for the source file.
            // After COW + rename, that inode must resolve to the upper destination.
            let upper_ino = self.assign_upper_ino(dst_path.clone());
            self.promoted.lock().unwrap().insert(attr.ino, upper_ino);
        }

        // Only track whiteouts for paths that exist in the lower layer.
        // Single lock acquisition for check + modify to avoid TOCTOU.
        let src_in_lower = self.lower_ino_for_path(&src_path).is_some();
        let mut state = self.state.lock().unwrap();
        let dst_whiteoutd = state.is_whiteout(&dst_path);
        let mut dirty = false;
        if src_in_lower {
            state.whiteouts.insert(src_path.clone());
            dirty = true;
        }
        if dst_whiteoutd {
            state.whiteouts.remove(&dst_path);
            dirty = true;
        }
        if dirty {
            state.flush().map_err(|_e| EIO)?;
        }
        drop(state);

        // For upper→upper renames, remap the existing inode to the new path.
        // The kernel keeps the source inode alive and expects it to resolve
        // to the new location.
        // For lower→upper (COW), the inode was already assigned above.
        if upper_src_existed {
            self.upper_inodes
                .lock()
                .unwrap()
                .rename_path(&src_path, dst_path);
        }
        Ok(())
    }

    fn setattr(&self, ino: u64, size: Option<u64>, mode: Option<u32>) -> Result<FileAttr, i32> {
        // Only COW for meaningful mutations (size/mode), not timestamp updates.
        // Combined with noatime mount option, this avoids copying large binaries
        // just because something read them.
        if size.is_none() && mode.is_none() {
            return self.getattr(ino);
        }

        let (virtual_path, upper_ino) = match self.resolve_ino(ino)? {
            ResolvedIno::Upper(path) => (path, ino),
            ResolvedIno::Lower(lower_ino, path) => {
                self.copy_to_upper(&path, lower_ino)?;
                let ui = self.assign_upper_ino(path.clone());
                self.promoted.lock().unwrap().insert(lower_ino, ui);
                (path, ui)
            }
        };

        let upper_path = self.upper_path(&virtual_path);

        if let Some(size) = size {
            let file = File::options()
                .write(true)
                .open(&upper_path)
                .map_err(|_e| EIO)?;
            file.set_len(size).map_err(|_e| EIO)?;
        }
        if let Some(mode) = mode {
            set_file_permissions(&upper_path, mode).map_err(|_e| EIO)?;
        }

        self.make_upper_attr(&virtual_path, upper_ino)
    }

    fn ino_to_path(&self, ino: u64) -> Result<PathBuf, i32> {
        match self.resolve_ino(ino)? {
            ResolvedIno::Upper(p) | ResolvedIno::Lower(_, p) => Ok(p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;
    use tempfile::TempDir;

    // A minimal VfsOps implementation for testing the overlay
    struct MockLowerFS;

    impl VfsOps for MockLowerFS {
        fn lookup(&self, _parent: u64, _name: &OsStr) -> Result<FileAttr, i32> {
            Err(ENOENT)
        }
        fn getattr(&self, _ino: u64) -> Result<FileAttr, i32> {
            Err(ENOENT)
        }
        fn readlink(&self, _ino: u64) -> Result<PathBuf, i32> {
            Err(ENOENT)
        }
        fn read(&self, _ino: u64, _offset: u64, _size: u32) -> Result<Vec<u8>, i32> {
            Err(EIO)
        }
        fn content_source(&self, _ino: u64) -> Result<ContentSource, i32> {
            Err(ENOENT)
        }
        fn readdir(&self, _ino: u64, _offset: u64) -> Result<Vec<DirEntry>, i32> {
            Ok(vec![])
        }
        fn ino_to_path(&self, ino: u64) -> Result<PathBuf, i32> {
            if ino == 1 {
                Ok(PathBuf::new()) // root
            } else {
                Err(ENOENT)
            }
        }
    }

    #[test]
    fn test_overlay_create_and_read() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(MockLowerFS, overlay_dir, "hash".into(), "test".into()).unwrap();

        // Create needs a parent inode. Use UPPER_INODE_BASE as a root-like parent.
        // First, create a directory for the parent
        let dir_attr = ofs.mkdir(1, OsStr::new("testdir"), 0o755).unwrap();

        // Create a file in that directory
        let (file_attr, fh) = ofs
            .create(dir_attr.ino, OsStr::new("test.txt"), 0o644)
            .unwrap();
        assert!(file_attr.ino >= UPPER_INODE_BASE);

        // Write content
        ofs.write(fh, 0, b"hello overlay").unwrap();

        // Read it back
        let data = ofs.read_handle(fh, 0, 1024).unwrap();
        assert_eq!(data, b"hello overlay");
    }

    #[test]
    fn test_overlay_unlink() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerFS,
            overlay_dir.clone(),
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Create a dir and file
        let dir_attr = ofs.mkdir(1, OsStr::new("dir"), 0o755).unwrap();
        ofs.create(dir_attr.ino, OsStr::new("file.txt"), 0o644)
            .unwrap();

        // Verify file exists
        let state = ofs.state.lock().unwrap();
        assert!(state.has_upper(Path::new("dir/file.txt")));
        drop(state);

        // Delete it
        ofs.unlink(dir_attr.ino, OsStr::new("file.txt")).unwrap();

        // File should be gone from upper. No whiteout needed since it was upper-only.
        let state = ofs.state.lock().unwrap();
        assert!(!state.has_upper(Path::new("dir/file.txt")));
        assert!(!state.is_whiteout(Path::new("dir/file.txt")));
    }

    #[test]
    fn test_overlay_create_after_unlink() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(MockLowerFS, overlay_dir, "hash".into(), "test".into()).unwrap();

        let dir_attr = ofs.mkdir(1, OsStr::new("dir"), 0o755).unwrap();

        // Create, delete, recreate
        ofs.create(dir_attr.ino, OsStr::new("file.txt"), 0o644)
            .unwrap();
        ofs.unlink(dir_attr.ino, OsStr::new("file.txt")).unwrap();
        let (_, fh) = ofs
            .create(dir_attr.ino, OsStr::new("file.txt"), 0o644)
            .unwrap();

        // Whiteout should be cleared
        let state = ofs.state.lock().unwrap();
        assert!(!state.is_whiteout(Path::new("dir/file.txt")));
        assert!(state.has_upper(Path::new("dir/file.txt")));
        drop(state);

        // Write and read
        ofs.write(fh, 0, b"recreated").unwrap();
        let data = ofs.read_handle(fh, 0, 1024).unwrap();
        assert_eq!(data, b"recreated");
    }

    #[test]
    fn test_overlay_mkdir_rmdir() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(MockLowerFS, overlay_dir, "hash".into(), "test".into()).unwrap();

        let dir_attr = ofs.mkdir(1, OsStr::new("mydir"), 0o755).unwrap();
        assert!(dir_attr.ino >= UPPER_INODE_BASE);

        let state = ofs.state.lock().unwrap();
        assert!(state.has_upper(Path::new("mydir")));
        drop(state);

        ofs.rmdir(1, OsStr::new("mydir")).unwrap();

        // No whiteout needed since this dir was upper-only (MockLowerFS has no files)
        let state = ofs.state.lock().unwrap();
        assert!(!state.is_whiteout(Path::new("mydir")));
    }

    #[test]
    fn test_overlay_getattr_upper() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(MockLowerFS, overlay_dir, "hash".into(), "test".into()).unwrap();

        let dir_attr = ofs.mkdir(1, OsStr::new("dir"), 0o755).unwrap();
        let (file_attr, fh) = ofs
            .create(dir_attr.ino, OsStr::new("test.txt"), 0o644)
            .unwrap();
        ofs.write(fh, 0, b"content").unwrap();

        // getattr should reflect the written size
        // Need to flush/close first for size to be accurate
        ofs.release_write(fh);

        let attr = ofs.getattr(file_attr.ino).unwrap();
        assert_eq!(attr.size, 7); // "content".len()
        assert_eq!(attr.kind, FileKind::RegularFile);
    }

    #[test]
    fn test_overlay_persistence() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");

        // Create files in first session
        {
            let ofs = OverlayFS::new(
                MockLowerFS,
                overlay_dir.clone(),
                "hash".into(),
                "test".into(),
            )
            .unwrap();
            let dir_attr = ofs.mkdir(1, OsStr::new("dir"), 0o755).unwrap();
            let (_, fh) = ofs
                .create(dir_attr.ino, OsStr::new("persist.txt"), 0o644)
                .unwrap();
            ofs.write(fh, 0, b"persistent").unwrap();
            ofs.release_write(fh);
        }

        // Reload and verify
        {
            let ofs =
                OverlayFS::new(MockLowerFS, overlay_dir, "hash".into(), "test".into()).unwrap();
            let state = ofs.state.lock().unwrap();
            assert!(state.has_upper(Path::new("dir/persist.txt")));
        }
    }

    // --- Merged directory tests using a richer mock ---

    /// A mock lower FS with a known file tree for testing merged lookups.
    /// Tree: root(1) → lib(2) → python(3) → foo.py(4), bar.py(5)
    struct MockLowerWithFiles;

    impl VfsOps for MockLowerWithFiles {
        fn lookup(&self, parent: u64, name: &OsStr) -> Result<FileAttr, i32> {
            let make_attr = |ino: u64, kind: FileKind, size: u64| FileAttr {
                ino,
                size,
                blocks: 1,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                kind,
                perm: 0o644,
                nlink: 1,
                uid: 0,
                gid: 0,
            };
            // Tree: root(1) → lib(2) → python(3) → {foo.py(4), bar.py(5)}
            //                → bin(6) → pytest(7)  [virtual entry point]
            match (parent, name.to_str().unwrap()) {
                (1, "lib") => Ok(make_attr(2, FileKind::Directory, 0)),
                (1, "bin") => Ok(make_attr(6, FileKind::Directory, 0)),
                (2, "python") => Ok(make_attr(3, FileKind::Directory, 0)),
                (3, "foo.py") => Ok(make_attr(4, FileKind::RegularFile, 11)),
                (3, "bar.py") => Ok(make_attr(5, FileKind::RegularFile, 11)),
                (6, "pytest") => Ok(make_attr(7, FileKind::RegularFile, 22)),
                _ => Err(ENOENT),
            }
        }
        fn getattr(&self, ino: u64) -> Result<FileAttr, i32> {
            let (kind, size) = match ino {
                1 | 2 | 3 | 6 => (FileKind::Directory, 0),
                4 | 5 => (FileKind::RegularFile, 11), // "foo content" / "bar content"
                7 => (FileKind::RegularFile, 22),     // "#!/bin/python\nimport..." (virtual)
                _ => return Err(ENOENT),
            };
            Ok(FileAttr {
                ino,
                size,
                blocks: 1,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                kind,
                perm: 0o644,
                nlink: 1,
                uid: 0,
                gid: 0,
            })
        }
        fn readlink(&self, _ino: u64) -> Result<PathBuf, i32> {
            Err(ENOENT)
        }
        fn read(&self, ino: u64, offset: u64, size: u32) -> Result<Vec<u8>, i32> {
            let content = match ino {
                4 => b"foo content".to_vec(),
                5 => b"bar content".to_vec(),
                7 => b"#!/bin/python\nimport x".to_vec(), // virtual entry point
                _ => return Err(EIO),
            };
            let start = offset as usize;
            let end = (start + size as usize).min(content.len());
            if start >= content.len() {
                return Ok(vec![]);
            }
            Ok(content[start..end].to_vec())
        }
        fn content_source(&self, ino: u64) -> Result<ContentSource, i32> {
            match ino {
                4 | 5 | 7 => Ok(ContentSource::Virtual), // entry point script
                _ => Err(ENOENT),
            }
        }
        fn readdir(&self, ino: u64, _offset: u64) -> Result<Vec<DirEntry>, i32> {
            match ino {
                1 => Ok(vec![
                    DirEntry {
                        ino: 2,
                        kind: FileKind::Directory,
                        name: "lib".into(),
                    },
                    DirEntry {
                        ino: 6,
                        kind: FileKind::Directory,
                        name: "bin".into(),
                    },
                ]),
                2 => Ok(vec![DirEntry {
                    ino: 3,
                    kind: FileKind::Directory,
                    name: "python".into(),
                }]),
                3 => Ok(vec![
                    DirEntry {
                        ino: 4,
                        kind: FileKind::RegularFile,
                        name: "foo.py".into(),
                    },
                    DirEntry {
                        ino: 5,
                        kind: FileKind::RegularFile,
                        name: "bar.py".into(),
                    },
                ]),
                6 => Ok(vec![DirEntry {
                    ino: 7,
                    kind: FileKind::RegularFile,
                    name: "pytest".into(),
                }]),
                _ => Err(ENOENT),
            }
        }
        fn ino_to_path(&self, ino: u64) -> Result<PathBuf, i32> {
            match ino {
                1 => Ok(PathBuf::new()),
                2 => Ok(PathBuf::from("lib")),
                3 => Ok(PathBuf::from("lib/python")),
                4 => Ok(PathBuf::from("lib/python/foo.py")),
                5 => Ok(PathBuf::from("lib/python/bar.py")),
                6 => Ok(PathBuf::from("bin")),
                7 => Ok(PathBuf::from("bin/pytest")),
                _ => Err(ENOENT),
            }
        }
    }

    #[test]
    fn test_overlay_lookup_falls_through_to_lower() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // lookup lib → lower inode 2
        let lib_attr = ofs.lookup(1, OsStr::new("lib")).unwrap();
        assert_eq!(lib_attr.ino, 2);

        // lookup python inside lib → lower inode 3
        let python_attr = ofs.lookup(lib_attr.ino, OsStr::new("python")).unwrap();
        assert_eq!(python_attr.ino, 3);

        // lookup foo.py inside python → lower inode 4
        let foo_attr = ofs.lookup(python_attr.ino, OsStr::new("foo.py")).unwrap();
        assert_eq!(foo_attr.ino, 4);
    }

    #[test]
    fn test_overlay_lookup_through_upper_parent_falls_to_lower() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir.clone(),
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Create a file in the upper layer under lib/python/
        // This creates lib/ and lib/python/ as structural dirs in the upper
        fs::create_dir_all(overlay_dir.join("lib/python")).unwrap();
        fs::write(overlay_dir.join("lib/python/new.py"), b"new").unwrap();

        // Assign upper inodes (simulating what readdir would do)
        ofs.assign_upper_ino(PathBuf::from("lib"));
        ofs.assign_upper_ino(PathBuf::from("lib/python"));
        ofs.assign_upper_ino(PathBuf::from("lib/python/new.py"));

        // Get the upper inode for lib/python
        let python_upper_ino = ofs
            .upper_inodes
            .lock()
            .unwrap()
            .ino_for_path(Path::new("lib/python"))
            .unwrap();

        // Looking up foo.py via the UPPER inode for lib/python should still
        // find it in the LOWER layer
        let foo_attr = ofs.lookup(python_upper_ino, OsStr::new("foo.py")).unwrap();
        assert_eq!(foo_attr.ino, 4); // lower inode
    }

    #[test]
    fn test_overlay_readdir_merges_upper_and_lower() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir.clone(),
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Add a new file in the upper layer under lib/python/
        fs::create_dir_all(overlay_dir.join("lib/python")).unwrap();
        fs::write(overlay_dir.join("lib/python/new.py"), b"new").unwrap();

        // readdir on lower inode 3 (lib/python) should merge both layers
        let entries = ofs.readdir(3, 0).unwrap();
        let names: Vec<String> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| e.name.to_str().unwrap().to_string())
            .collect();

        assert!(
            names.contains(&"foo.py".to_string()),
            "missing lower file foo.py"
        );
        assert!(
            names.contains(&"bar.py".to_string()),
            "missing lower file bar.py"
        );
        assert!(
            names.contains(&"new.py".to_string()),
            "missing upper file new.py"
        );
    }

    #[test]
    fn test_overlay_opaque_dir_hides_lower_contents() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // lib/python exists in lower with foo.py and bar.py
        let lib_attr = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let _python_attr = ofs.lookup(lib_attr.ino, OsStr::new("python")).unwrap();

        // Whiteout all children first so rmdir succeeds (POSIX semantics)
        ofs.unlink(3, OsStr::new("foo.py")).unwrap();
        ofs.unlink(3, OsStr::new("bar.py")).unwrap();

        // Remove and recreate lib/python — should become opaque
        ofs.rmdir(2, OsStr::new("python")).unwrap();
        let new_python = ofs.mkdir(2, OsStr::new("python"), 0o755).unwrap();

        // Lower files should be hidden — lookup foo.py should fail
        assert!(ofs.lookup(new_python.ino, OsStr::new("foo.py")).is_err());

        // readdir should only show . and .. (no lower files)
        let entries = ofs.readdir(new_python.ino, 0).unwrap();
        let names: Vec<String> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| e.name.to_str().unwrap().to_string())
            .collect();
        assert!(names.is_empty(), "expected empty dir, got: {names:?}");

        // Creating a new file in the opaque dir should work
        let (_, fh) = ofs
            .create(new_python.ino, OsStr::new("new.py"), 0o644)
            .unwrap();
        ofs.write(fh, 0, b"fresh").unwrap();

        let entries = ofs.readdir(new_python.ino, 0).unwrap();
        let names: Vec<String> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| e.name.to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["new.py"]);
    }

    #[test]
    fn test_overlay_readdir_filters_whiteouts() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Delete foo.py (lower file) via whiteout
        ofs.unlink(3, OsStr::new("foo.py")).unwrap();

        // readdir should no longer include foo.py
        let entries = ofs.readdir(3, 0).unwrap();
        let names: Vec<String> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| e.name.to_str().unwrap().to_string())
            .collect();

        assert!(
            !names.contains(&"foo.py".to_string()),
            "foo.py should be hidden by whiteout"
        );
        assert!(
            names.contains(&"bar.py".to_string()),
            "bar.py should still be visible"
        );
    }

    // --- Rename and inode promotion tests ---

    #[test]
    fn test_rename_upper_to_upper() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(MockLowerFS, overlay_dir, "hash".into(), "test".into()).unwrap();

        let dir_attr = ofs.mkdir(1, OsStr::new("d"), 0o755).unwrap();
        let (file_attr, fh) = ofs
            .create(dir_attr.ino, OsStr::new("a.txt"), 0o644)
            .unwrap();
        ofs.write(fh, 0, b"hello").unwrap();
        ofs.release_write(fh);

        let original_ino = file_attr.ino;

        // Rename a.txt → b.txt
        ofs.rename(
            dir_attr.ino,
            OsStr::new("a.txt"),
            dir_attr.ino,
            OsStr::new("b.txt"),
            0,
        )
        .unwrap();

        // Kernel uses the original inode — getattr should still work
        let attr = ofs.getattr(original_ino).unwrap();
        assert_eq!(attr.size, 5);

        // read via original inode should return content
        let data = ofs.read(original_ino, 0, 1024).unwrap();
        assert_eq!(data, b"hello");

        // lookup new name should succeed
        assert!(ofs.lookup(dir_attr.ino, OsStr::new("b.txt")).is_ok());

        // lookup old name should fail
        assert_eq!(
            ofs.lookup(dir_attr.ino, OsStr::new("a.txt")).unwrap_err(),
            ENOENT
        );
    }

    #[test]
    fn test_rename_lower_to_upper() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Get lower inode for foo.py (ino=4) via lookup chain
        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();
        let foo = ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap();
        assert_eq!(foo.ino, 4); // lower inode

        // Rename foo.py → moved.py
        ofs.rename(
            python.ino,
            OsStr::new("foo.py"),
            python.ino,
            OsStr::new("moved.py"),
            0,
        )
        .unwrap();

        // getattr with the LOWER inode (4) should still work via promoted map
        let attr = ofs.getattr(4).unwrap();
        assert_eq!(attr.kind, FileKind::RegularFile);

        // read with lower inode should return the COW'd content
        let data = ofs.read(4, 0, 1024).unwrap();
        assert_eq!(data, b"foo content");

        // lookup new name should succeed
        assert!(ofs.lookup(python.ino, OsStr::new("moved.py")).is_ok());

        // lookup old name should fail (whiteout)
        assert_eq!(
            ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap_err(),
            ENOENT
        );

        // Whiteout should be persisted
        let state = ofs.state.lock().unwrap();
        assert!(state.is_whiteout(Path::new("lib/python/foo.py")));
    }

    #[test]
    fn test_rename_noreplace_existing() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(MockLowerFS, overlay_dir, "hash".into(), "test".into()).unwrap();

        let dir_attr = ofs.mkdir(1, OsStr::new("d"), 0o755).unwrap();
        ofs.create(dir_attr.ino, OsStr::new("a.txt"), 0o644)
            .unwrap();
        ofs.create(dir_attr.ino, OsStr::new("b.txt"), 0o644)
            .unwrap();

        // RENAME_NOREPLACE should fail when destination exists
        #[cfg(target_os = "linux")]
        {
            let err = ofs
                .rename(
                    dir_attr.ino,
                    OsStr::new("a.txt"),
                    dir_attr.ino,
                    OsStr::new("b.txt"),
                    libc::RENAME_NOREPLACE,
                )
                .unwrap_err();
            assert_eq!(err, libc::EEXIST);
        }
    }

    #[test]
    fn test_rename_noreplace_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(MockLowerFS, overlay_dir, "hash".into(), "test".into()).unwrap();

        let dir_attr = ofs.mkdir(1, OsStr::new("d"), 0o755).unwrap();
        ofs.create(dir_attr.ino, OsStr::new("a.txt"), 0o644)
            .unwrap();

        // RENAME_NOREPLACE should succeed when destination doesn't exist
        #[cfg(target_os = "linux")]
        ofs.rename(
            dir_attr.ino,
            OsStr::new("a.txt"),
            dir_attr.ino,
            OsStr::new("new.txt"),
            libc::RENAME_NOREPLACE,
        )
        .unwrap();
    }

    #[test]
    fn test_rename_noreplace_lower_existing() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Create an upper file
        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();
        ofs.create(python.ino, OsStr::new("new.py"), 0o644).unwrap();

        // RENAME_NOREPLACE to a lower-layer file should fail
        #[cfg(target_os = "linux")]
        {
            let err = ofs
                .rename(
                    python.ino,
                    OsStr::new("new.py"),
                    python.ino,
                    OsStr::new("foo.py"), // exists in lower
                    libc::RENAME_NOREPLACE,
                )
                .unwrap_err();
            assert_eq!(err, libc::EEXIST);
        }
    }

    #[test]
    fn test_rename_chain() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Get lower inode for foo.py
        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();
        let foo = ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap();
        let original_ino = foo.ino; // lower inode 4

        // Chain: foo.py → temp.py → final.py
        ofs.rename(
            python.ino,
            OsStr::new("foo.py"),
            python.ino,
            OsStr::new("temp.py"),
            0,
        )
        .unwrap();
        ofs.rename(
            python.ino,
            OsStr::new("temp.py"),
            python.ino,
            OsStr::new("final.py"),
            0,
        )
        .unwrap();

        // Original lower inode should resolve through promoted → upper → current path
        let attr = ofs.getattr(original_ino).unwrap();
        assert_eq!(attr.kind, FileKind::RegularFile);

        let data = ofs.read(original_ino, 0, 1024).unwrap();
        assert_eq!(data, b"foo content");

        // Only final.py should be findable
        assert!(ofs.lookup(python.ino, OsStr::new("final.py")).is_ok());
        assert_eq!(
            ofs.lookup(python.ino, OsStr::new("temp.py")).unwrap_err(),
            ENOENT
        );
        assert_eq!(
            ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap_err(),
            ENOENT
        );
    }

    #[test]
    fn test_rename_overwrite_upper() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(MockLowerFS, overlay_dir, "hash".into(), "test".into()).unwrap();

        let dir_attr = ofs.mkdir(1, OsStr::new("d"), 0o755).unwrap();

        // Create source and destination files
        let (_, fh_a) = ofs
            .create(dir_attr.ino, OsStr::new("a.txt"), 0o644)
            .unwrap();
        ofs.write(fh_a, 0, b"source").unwrap();
        ofs.release_write(fh_a);

        let (dst_attr, fh_b) = ofs
            .create(dir_attr.ino, OsStr::new("b.txt"), 0o644)
            .unwrap();
        ofs.write(fh_b, 0, b"destination").unwrap();
        ofs.release_write(fh_b);

        let src_ino = ofs.lookup(dir_attr.ino, OsStr::new("a.txt")).unwrap().ino;
        let _dst_ino = dst_attr.ino;

        // Rename a.txt → b.txt (overwrite)
        ofs.rename(
            dir_attr.ino,
            OsStr::new("a.txt"),
            dir_attr.ino,
            OsStr::new("b.txt"),
            0,
        )
        .unwrap();

        // Source inode should now have the source content at the new path
        let data = ofs.read(src_ino, 0, 1024).unwrap();
        assert_eq!(data, b"source");

        // lookup b.txt should return source inode
        let b_attr = ofs.lookup(dir_attr.ino, OsStr::new("b.txt")).unwrap();
        assert_eq!(b_attr.ino, src_ino);
    }

    #[test]
    fn test_open_write_promotes_lower_inode() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Get lower inode for foo.py
        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();
        let foo = ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap();
        assert_eq!(foo.ino, 4); // lower inode

        // open_write triggers COW
        let fh = ofs.open_write(4).unwrap();
        ofs.write(fh, 0, b"modified content").unwrap();
        ofs.release_write(fh);

        // getattr with lower inode should reflect the upper copy
        let attr = ofs.getattr(4).unwrap();
        assert_eq!(attr.size, 16); // "modified content".len()

        // read with lower inode should return modified content
        let data = ofs.read(4, 0, 1024).unwrap();
        assert_eq!(data, b"modified content");
    }

    #[test]
    fn test_unlink_virtual_lower_file() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Lookup bin directory and the virtual entry point
        let bin = ofs.lookup(1, OsStr::new("bin")).unwrap();
        let pytest = ofs.lookup(bin.ino, OsStr::new("pytest")).unwrap();
        assert_eq!(pytest.ino, 7); // lower inode

        // Read the virtual file content
        let data = ofs.read(pytest.ino, 0, 1024).unwrap();
        assert_eq!(data, b"#!/bin/python\nimport x");

        // Unlink the virtual file (should create a whiteout)
        ofs.unlink(bin.ino, OsStr::new("pytest")).unwrap();

        // Lookup should now fail
        assert_eq!(
            ofs.lookup(bin.ino, OsStr::new("pytest")).unwrap_err(),
            ENOENT
        );

        // Whiteout should be recorded
        let state = ofs.state.lock().unwrap();
        assert!(state.is_whiteout(Path::new("bin/pytest")));
    }

    #[test]
    fn test_rename_virtual_lower_file() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // Lookup bin directory and the virtual entry point
        let bin = ofs.lookup(1, OsStr::new("bin")).unwrap();
        let pytest = ofs.lookup(bin.ino, OsStr::new("pytest")).unwrap();

        // Rename the virtual file within the same directory
        ofs.rename(
            bin.ino,
            OsStr::new("pytest"),
            bin.ino,
            OsStr::new("pytest.bak"),
            0,
        )
        .unwrap();

        // Original name should be gone (whiteout)
        assert_eq!(
            ofs.lookup(bin.ino, OsStr::new("pytest")).unwrap_err(),
            ENOENT
        );

        // New name should work — getattr with original lower inode via promoted map
        let attr = ofs.getattr(pytest.ino).unwrap();
        assert_eq!(attr.kind, FileKind::RegularFile);

        // Read via the promoted inode should return the virtual content
        let data = ofs.read(pytest.ino, 0, 1024).unwrap();
        assert_eq!(data, b"#!/bin/python\nimport x");

        // Lookup new name should succeed
        assert!(ofs.lookup(bin.ino, OsStr::new("pytest.bak")).is_ok());
    }

    #[test]
    fn test_rename_lower_directory_preserves_children() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir.clone(),
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // MockLowerWithFiles: lib(2) → python(3) → {foo.py(4), bar.py(5)}
        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let _python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();

        // Rename lib/python → lib/~python (simulates pip's stash rename)
        ofs.rename(
            lib.ino,
            OsStr::new("python"),
            lib.ino,
            OsStr::new("~python"),
            0,
        )
        .unwrap();

        // The renamed directory should be a real directory, not a 0-byte file
        let upper_renamed = overlay_dir.join("lib/~python");
        assert!(
            upper_renamed.is_dir(),
            "renamed dir should be a directory on disk"
        );

        // Children should be accessible under the new name
        let renamed = ofs.lookup(lib.ino, OsStr::new("~python")).unwrap();
        let foo = ofs.lookup(renamed.ino, OsStr::new("foo.py")).unwrap();
        let data = ofs.read(foo.ino, 0, 1024).unwrap();
        assert_eq!(data, b"foo content");

        let bar = ofs.lookup(renamed.ino, OsStr::new("bar.py")).unwrap();
        let data = ofs.read(bar.ino, 0, 1024).unwrap();
        assert_eq!(data, b"bar content");

        // Old path should be gone
        assert_eq!(
            ofs.lookup(lib.ino, OsStr::new("python")).unwrap_err(),
            ENOENT
        );
    }

    #[test]
    fn test_rmdir_lower_dir_with_remaining_children_fails() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // MockLowerWithFiles has: bin(6) → pytest(7)
        // Verify pytest is visible
        let bin = ofs.lookup(1, OsStr::new("bin")).unwrap();
        assert_eq!(bin.ino, 6);
        let pytest = ofs.lookup(bin.ino, OsStr::new("pytest")).unwrap();
        assert_eq!(pytest.ino, 7);

        // rmdir on a lower-layer directory that still has visible children
        // should fail with ENOTEMPTY — just like POSIX requires.
        let err = ofs.rmdir(1, OsStr::new("bin")).unwrap_err();
        assert_eq!(err, libc::ENOTEMPTY);

        // bin/pytest should still be accessible
        let bin = ofs.lookup(1, OsStr::new("bin")).unwrap();
        let pytest = ofs.lookup(bin.ino, OsStr::new("pytest")).unwrap();
        assert_eq!(pytest.ino, 7);
    }

    #[test]
    fn test_unlink_lower_dir_with_remaining_children_fails() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // MockLowerWithFiles has: bin(6) → pytest(7)
        // The NFS server routes RMDIR through unlink, so unlink must also
        // reject removing a directory with visible lower-layer children.
        let err = ofs.unlink(1, OsStr::new("bin")).unwrap_err();
        assert_eq!(err, libc::ENOTEMPTY);

        // bin/pytest should still be accessible
        let bin = ofs.lookup(1, OsStr::new("bin")).unwrap();
        let pytest = ofs.lookup(bin.ino, OsStr::new("pytest")).unwrap();
        assert_eq!(pytest.ino, 7);
    }

    #[test]
    fn test_rmdir_lower_dir_after_all_children_whiteoutd() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // MockLowerWithFiles has: bin(6) → pytest(7)
        // Whiteout the only child first
        ofs.unlink(6, OsStr::new("pytest")).unwrap();

        // Now rmdir should succeed — all children are whiteoutd
        ofs.rmdir(1, OsStr::new("bin")).unwrap();

        // bin should now be invisible
        assert_eq!(ofs.lookup(1, OsStr::new("bin")).unwrap_err(), ENOENT);
    }

    /// Renaming a directory that has content in BOTH layers should preserve
    /// all children at the destination — not just the upper-layer ones.
    /// Real-world: Python creates __pycache__/ in upper, then pip renames
    /// the whole package directory.
    #[test]
    fn test_rename_dir_with_both_upper_and_lower_content() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir.clone(),
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // MockLowerWithFiles: lib(2) → python(3) → {foo.py(4), bar.py(5)}
        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();

        // Simulate Python creating a __pycache__/ dir + .pyc in upper
        let pycache = ofs
            .mkdir(python.ino, OsStr::new("__pycache__"), 0o755)
            .unwrap();
        let (_, fh) = ofs
            .create(pycache.ino, OsStr::new("foo.cpython-312.pyc"), 0o644)
            .unwrap();
        ofs.write(fh, 0, b"compiled").unwrap();
        ofs.release_write(fh);

        // Now upper has lib/python/__pycache__/foo.cpython-312.pyc
        // Lower has lib/python/foo.py and lib/python/bar.py
        // Rename lib/python → lib/~python (pip's stash pattern)
        ofs.rename(
            lib.ino,
            OsStr::new("python"),
            lib.ino,
            OsStr::new("~python"),
            0,
        )
        .unwrap();

        // The renamed directory should have ALL content — both layers
        let renamed = ofs.lookup(lib.ino, OsStr::new("~python")).unwrap();

        // Lower-layer files must be accessible
        let foo = ofs.lookup(renamed.ino, OsStr::new("foo.py")).unwrap();
        let data = ofs.read(foo.ino, 0, 1024).unwrap();
        assert_eq!(data, b"foo content");

        let bar = ofs.lookup(renamed.ino, OsStr::new("bar.py")).unwrap();
        let data = ofs.read(bar.ino, 0, 1024).unwrap();
        assert_eq!(data, b"bar content");

        // Upper-layer content should also be there
        let pc = ofs.lookup(renamed.ino, OsStr::new("__pycache__")).unwrap();
        assert!(
            ofs.lookup(pc.ino, OsStr::new("foo.cpython-312.pyc"))
                .is_ok()
        );

        // Old path should be gone
        assert_eq!(
            ofs.lookup(lib.ino, OsStr::new("python")).unwrap_err(),
            ENOENT
        );
    }

    /// Renaming a file onto a path that exists in the lower layer should
    /// prevent the lower file from bleeding through after the upper file
    /// is later deleted.
    #[test]
    fn test_rename_overwrite_lower_then_unlink() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();

        // Create a new upper file
        let (_, fh) = ofs.create(python.ino, OsStr::new("new.py"), 0o644).unwrap();
        ofs.write(fh, 0, b"new content").unwrap();
        ofs.release_write(fh);

        // Rename new.py → foo.py, overwriting the lower-layer foo.py
        ofs.rename(
            python.ino,
            OsStr::new("new.py"),
            python.ino,
            OsStr::new("foo.py"),
            0,
        )
        .unwrap();

        // foo.py should have the new content
        let foo = ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap();
        let data = ofs.read(foo.ino, 0, 1024).unwrap();
        assert_eq!(data, b"new content");

        // Now unlink foo.py — the lower-layer original should NOT bleed through
        ofs.unlink(python.ino, OsStr::new("foo.py")).unwrap();
        assert_eq!(
            ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap_err(),
            ENOENT
        );
    }

    /// setattr on a lower-layer file should COW it to upper before modifying.
    #[test]
    fn test_setattr_cow_lower_file() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir.clone(),
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();
        let foo = ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap();
        assert_eq!(foo.ino, 4); // lower inode

        // Truncate the file — should COW to upper
        let attr = ofs.setattr(foo.ino, Some(5), None).unwrap();
        assert_eq!(attr.size, 5);

        // The COW'd file should exist in upper
        assert!(overlay_dir.join("lib/python/foo.py").exists());

        // Read should return truncated content
        let data = ofs.read(foo.ino, 0, 1024).unwrap();
        assert_eq!(data, b"foo c");
    }

    /// setattr with no meaningful changes should not trigger COW.
    #[test]
    fn test_setattr_noop_no_cow() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir.clone(),
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();
        let _foo = ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap();

        // setattr with no size/mode should be a no-op
        ofs.setattr(4, None, None).unwrap();

        // File should NOT have been COW'd to upper
        assert!(!overlay_dir.join("lib/python/foo.py").exists());
    }

    /// Rename a file across different parent directories.
    #[test]
    fn test_rename_across_directories() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();
        let bin = ofs.lookup(1, OsStr::new("bin")).unwrap();

        // Move foo.py from lib/python/ to bin/
        ofs.rename(
            python.ino,
            OsStr::new("foo.py"),
            bin.ino,
            OsStr::new("foo.py"),
            0,
        )
        .unwrap();

        // Should be gone from python/
        assert_eq!(
            ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap_err(),
            ENOENT
        );

        // Should exist in bin/
        let foo = ofs.lookup(bin.ino, OsStr::new("foo.py")).unwrap();
        let data = ofs.read(foo.ino, 0, 1024).unwrap();
        assert_eq!(data, b"foo content");
    }

    /// readdir should reflect mutations (create, unlink, rename).
    #[test]
    fn test_readdir_reflects_mutations() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();

        let get_names = |ino: u64| -> Vec<String> {
            ofs.readdir(ino, 0)
                .unwrap()
                .iter()
                .filter(|e| e.name != "." && e.name != "..")
                .map(|e| e.name.to_str().unwrap().to_string())
                .collect()
        };

        // Initial state: foo.py, bar.py
        let names = get_names(python.ino);
        assert!(names.contains(&"foo.py".to_string()));
        assert!(names.contains(&"bar.py".to_string()));
        assert_eq!(names.len(), 2);

        // After unlink foo.py
        ofs.unlink(python.ino, OsStr::new("foo.py")).unwrap();
        let names = get_names(python.ino);
        assert!(!names.contains(&"foo.py".to_string()));
        assert!(names.contains(&"bar.py".to_string()));

        // After create new.py
        let (_, fh) = ofs.create(python.ino, OsStr::new("new.py"), 0o644).unwrap();
        ofs.release_write(fh);
        let names = get_names(python.ino);
        assert!(names.contains(&"new.py".to_string()));
        assert!(names.contains(&"bar.py".to_string()));
        assert!(!names.contains(&"foo.py".to_string()));

        // After rename bar.py → moved.py
        ofs.rename(
            python.ino,
            OsStr::new("bar.py"),
            python.ino,
            OsStr::new("moved.py"),
            0,
        )
        .unwrap();
        let names = get_names(python.ino);
        assert!(names.contains(&"new.py".to_string()));
        assert!(names.contains(&"moved.py".to_string()));
        assert!(!names.contains(&"bar.py".to_string()));
        assert!(!names.contains(&"foo.py".to_string()));
    }

    #[test]
    fn test_uninstall_reinstall_uninstall_cycle() {
        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");
        let ofs = OverlayFS::new(
            MockLowerWithFiles,
            overlay_dir,
            "hash".into(),
            "test".into(),
        )
        .unwrap();

        // MockLowerWithFiles: lib(2) → python(3) → {foo.py(4), bar.py(5)}
        let lib = ofs.lookup(1, OsStr::new("lib")).unwrap();
        let python = ofs.lookup(lib.ino, OsStr::new("python")).unwrap();

        // -- Step 1: "pip uninstall" the old version --
        // Verify originals are visible
        let foo = ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap();
        assert_eq!(ofs.read(foo.ino, 0, 1024).unwrap(), b"foo content");

        // Remove them (creates whiteouts)
        ofs.unlink(python.ino, OsStr::new("foo.py")).unwrap();
        ofs.unlink(python.ino, OsStr::new("bar.py")).unwrap();

        assert_eq!(
            ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap_err(),
            ENOENT
        );
        assert_eq!(
            ofs.lookup(python.ino, OsStr::new("bar.py")).unwrap_err(),
            ENOENT
        );

        // -- Step 2: "pip install" a new version at the same paths --
        let (new_foo_attr, fh) = ofs.create(python.ino, OsStr::new("foo.py"), 0o644).unwrap();
        ofs.write(fh, 0, b"NEW foo content v2").unwrap();
        ofs.release_write(fh);

        let (_, fh) = ofs.create(python.ino, OsStr::new("bar.py"), 0o644).unwrap();
        ofs.write(fh, 0, b"NEW bar content v2").unwrap();
        ofs.release_write(fh);

        // -- Step 3: Verify the new version is what we see --
        let new_foo = ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap();
        assert_eq!(new_foo.ino, new_foo_attr.ino); // upper inode, not lower
        let data = ofs.read(new_foo.ino, 0, 1024).unwrap();
        assert_eq!(data, b"NEW foo content v2");

        // readdir should show the new files, not the old ones
        let entries = ofs.readdir(python.ino, 0).unwrap();
        let names: Vec<_> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| e.name.to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"foo.py".to_string()));
        assert!(names.contains(&"bar.py".to_string()));

        // -- Step 4: "pip uninstall" the new version --
        ofs.unlink(python.ino, OsStr::new("foo.py")).unwrap();
        ofs.unlink(python.ino, OsStr::new("bar.py")).unwrap();

        // -- Step 5: Verify lower-layer originals do NOT bleed through --
        assert_eq!(
            ofs.lookup(python.ino, OsStr::new("foo.py")).unwrap_err(),
            ENOENT
        );
        assert_eq!(
            ofs.lookup(python.ino, OsStr::new("bar.py")).unwrap_err(),
            ENOENT
        );

        // readdir should be empty (whiteouts still active)
        let entries = ofs.readdir(python.ino, 0).unwrap();
        let names: Vec<_> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .collect();
        assert!(names.is_empty(), "lower layer bled through: {names:?}");

        // Whiteouts should be persisted
        let state = ofs.state.lock().unwrap();
        assert!(state.is_whiteout(Path::new("lib/python/foo.py")));
        assert!(state.is_whiteout(Path::new("lib/python/bar.py")));
    }

    // --- B1/B2 regression tests: state validation is decoupled from VFS consumption ---

    #[test]
    fn test_load_then_wrap_recovers_from_env_hash_mismatch() {
        // Verifies the wipe-and-reload mechanism works at the OverlayState
        // level. create_overlay uses this for VersionMismatch (internal schema
        // changes). EnvHashMismatch returns a structured error to the caller
        // instead of auto-wiping, since the overlay may contain user work.
        use crate::overlay::{OverlayError, OverlayState};

        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");

        // Mount with hash A, then drop.
        {
            let _ofs = OverlayFS::new(
                MockLowerFS,
                overlay_dir.clone(),
                "hash_a".into(),
                "test".into(),
            )
            .unwrap();
        }

        // Try to load with hash B — should fail with EnvHashMismatch.
        let err = OverlayState::load(
            overlay_dir.clone(),
            "hash_b".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap_err();
        assert!(matches!(err, OverlayError::EnvHashMismatch { .. }));

        // Wipe and reload — this is the create_overlay recovery path.
        std::fs::remove_dir_all(&overlay_dir).unwrap();
        let state = OverlayState::load(
            overlay_dir.clone(),
            "hash_b".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();

        // wrap() consumes a fresh lower VFS — no double-construction needed.
        let ofs = OverlayFS::wrap(MockLowerFS, state).unwrap();
        assert!(!ofs.state.lock().unwrap().is_whiteout(Path::new("any")));
    }

    #[test]
    fn test_load_refuses_transport_mismatch() {
        // B2: TransportMismatch is a distinct typed error. The create_overlay
        // flow refuses to wipe on this variant — only EnvHashMismatch and
        // VersionMismatch trigger the wipe-and-retry recovery path.
        use crate::overlay::{OverlayError, OverlayState};

        let tmp = TempDir::new().unwrap();
        let overlay_dir = tmp.path().join("upper");

        // Mount with transport "fuse".
        {
            let _ofs = OverlayFS::new(
                MockLowerFS,
                overlay_dir.clone(),
                "hash".into(),
                "fuse".into(),
            )
            .unwrap();
        }

        // Try to load as "nfs" — should refuse with TransportMismatch.
        let err = OverlayState::load(
            overlay_dir.clone(),
            "hash".into(),
            "nfs".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap_err();
        assert!(matches!(err, OverlayError::TransportMismatch { .. }));

        // The overlay directory must still exist — no silent wipe.
        assert!(overlay_dir.exists());
        assert!(overlay_dir.join(".rattler_vfs_state.json").exists());
    }
}
