//! Inode bookkeeping for the writable overlay.
//!
//! The lower layer (read-only VFS) owns inodes `1..UPPER_INODE_BASE`. Upper
//! layer (overlay) entries get inodes `UPPER_INODE_BASE..u64::MAX` assigned
//! lazily by [`UpperInodeMap`]. [`ResolvedIno`] wraps the result of "given
//! this kernel-visible inode, which layer is it actually in right now?",
//! since rename and copy-on-write can promote a lower inode into the upper
//! layer dynamically.
//!
//! Extracted from `overlay_fs.rs` to keep the inode allocator and the COW /
//! `VfsOps` machinery in separate modules. Pure data + bookkeeping; no I/O.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Inodes at or above this value belong to the upper (overlay) layer.
/// Anything below is owned by the lower read-only VFS.
pub(crate) const UPPER_INODE_BASE: u64 = u64::MAX / 2;

/// Bidirectional inode ↔ path mapping for upper-layer entries.
pub(crate) struct UpperInodeMap {
    path_to_ino: HashMap<PathBuf, u64>,
    ino_to_path: HashMap<u64, PathBuf>,
    next_ino: AtomicU64,
}

impl UpperInodeMap {
    pub(crate) fn new() -> Self {
        Self {
            path_to_ino: HashMap::new(),
            ino_to_path: HashMap::new(),
            next_ino: AtomicU64::new(UPPER_INODE_BASE),
        }
    }

    pub(crate) fn get_or_assign(&mut self, virtual_path: PathBuf) -> u64 {
        if let Some(&ino) = self.path_to_ino.get(&virtual_path) {
            return ino;
        }
        let ino = self.next_ino.fetch_add(1, Ordering::Relaxed);
        self.ino_to_path.insert(ino, virtual_path.clone());
        self.path_to_ino.insert(virtual_path, ino);
        ino
    }

    pub(crate) fn path_for(&self, ino: u64) -> Option<&PathBuf> {
        self.ino_to_path.get(&ino)
    }

    /// Reverse lookup. Used by tests; production code asks the upper map
    /// via `path_for` after promotion.
    #[cfg(test)]
    pub(crate) fn ino_for_path(&self, virtual_path: &Path) -> Option<u64> {
        self.path_to_ino.get(virtual_path).copied()
    }

    /// Remap an existing inode from `old_path` to `new_path`.
    /// The kernel expects the source inode to remain valid after rename,
    /// just pointing at the new path.
    pub(crate) fn rename_path(&mut self, old_path: &Path, new_path: PathBuf) {
        // Clean up any existing inode at the destination (overwrite case)
        if let Some(old_dst_ino) = self.path_to_ino.remove(&new_path) {
            self.ino_to_path.remove(&old_dst_ino);
        }
        // Remap source inode to destination path
        if let Some(ino) = self.path_to_ino.remove(old_path) {
            self.ino_to_path.insert(ino, new_path.clone());
            self.path_to_ino.insert(new_path, ino);
        }
    }
}

/// Result of resolving a kernel-visible inode to its current backing layer.
pub(crate) enum ResolvedIno {
    /// File is in the upper (overlay) layer at this virtual path.
    Upper(PathBuf),
    /// File is in the lower (read-only) layer with this inode and virtual path.
    Lower(u64, PathBuf),
}
