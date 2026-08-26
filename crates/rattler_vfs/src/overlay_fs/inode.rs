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
};

/// Inodes at or above this value belong to the upper (overlay) layer.
/// Anything below is owned by the lower read-only VFS.
pub(crate) const UPPER_INODE_BASE: u64 = u64::MAX / 2;

/// Bidirectional inode ↔ path mapping for upper-layer entries.
pub(crate) struct UpperInodeMap {
    path_to_ino: HashMap<PathBuf, u64>,
    /// Reverse map, indexed by `ino - UPPER_INODE_BASE`. Upper inodes are handed
    /// out sequentially from `UPPER_INODE_BASE`, so a `Vec` is denser and faster
    /// than a `HashMap`. A slot is `None` once a rename orphans its inode (the
    /// overwritten destination — see [`Self::rename_path`]).
    ino_to_path: Vec<Option<PathBuf>>,
}

impl UpperInodeMap {
    pub(crate) fn new() -> Self {
        Self {
            path_to_ino: HashMap::new(),
            ino_to_path: Vec::new(),
        }
    }

    pub(crate) fn get_or_assign(&mut self, virtual_path: PathBuf) -> u64 {
        if let Some(&ino) = self.path_to_ino.get(&virtual_path) {
            return ino;
        }
        // Next sequential inode = base + number already assigned.
        let ino = UPPER_INODE_BASE + self.ino_to_path.len() as u64;
        self.ino_to_path.push(Some(virtual_path.clone()));
        self.path_to_ino.insert(virtual_path, ino);
        ino
    }

    pub(crate) fn path_for(&self, ino: u64) -> Option<&PathBuf> {
        let idx = ino.checked_sub(UPPER_INODE_BASE)? as usize;
        self.ino_to_path.get(idx)?.as_ref()
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
        // Clean up any existing inode at the destination (overwrite case):
        // orphan its slot rather than shifting the Vec, so other inodes keep
        // their indices.
        if let Some(old_dst_ino) = self.path_to_ino.remove(&new_path)
            && let Some(slot) = self.slot_mut(old_dst_ino)
        {
            *slot = None;
        }
        // Remap source inode to destination path
        if let Some(ino) = self.path_to_ino.remove(old_path) {
            if let Some(slot) = self.slot_mut(ino) {
                *slot = Some(new_path.clone());
            }
            self.path_to_ino.insert(new_path, ino);
        }
    }

    /// Mutable access to the reverse-map slot for an upper inode, if in range.
    fn slot_mut(&mut self, ino: u64) -> Option<&mut Option<PathBuf>> {
        let idx = ino.checked_sub(UPPER_INODE_BASE)? as usize;
        self.ino_to_path.get_mut(idx)
    }
}

/// Result of resolving a kernel-visible inode to its current backing layer.
pub(crate) enum ResolvedIno {
    /// File is in the upper (overlay) layer at this virtual path.
    Upper(PathBuf),
    /// File is in the lower (read-only) layer with this inode and virtual path.
    Lower(u64, PathBuf),
}
