use libc::{EIO, ENOENT, ENOTDIR};
use memmap2::Mmap;
#[cfg(target_os = "macos")]
use rattler::install::link::copy_and_replace_placeholders_with_offsets;
use rattler_conda_types::Platform;
use rattler_conda_types::package::{FileMode, OffsetRanges, PathType, select_utf8_offset_ranges};
#[cfg(target_os = "macos")]
use rattler_conda_types::package::{OffsetEncoding, OffsetGroup};
use std::{
    collections::{HashMap, VecDeque},
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
    time::UNIX_EPOCH,
};

use crate::vfs_ops::current_uid_gid;

use crate::metadata_tree::{FileNode, MetadataNode};
use crate::vfs_ops::{ContentSource, DirEntry, FileAttr, FileKind, VfsOps};

/// Compare a directory entry's name against a lookup name. Case-sensitive on
/// Unix; case-insensitive on Windows, where NTFS/ProjFS resolve paths
/// case-insensitively (a lookup for `Lib\\Foo` must find the stored `lib/foo`).
/// Conda paths are ASCII, so ASCII case folding is sufficient.
fn names_match(entry_name: &OsStr, lookup_name: &OsStr) -> bool {
    #[cfg(windows)]
    {
        entry_name
            .to_string_lossy()
            .eq_ignore_ascii_case(&lookup_name.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        entry_name == lookup_name
    }
}

/// Bounded FIFO cache of fully materialized + ad-hoc re-signed binaries (macOS).
///
/// Each entry is a whole binary, so an unbounded map would pin the entire
/// re-signed binary set in memory for the sidecar's lifetime. This caps the
/// entry count and evicts oldest-first; an evicted binary is simply recomputed
/// on the next read.
struct CodesignCache {
    map: HashMap<u64, Vec<u8>>,
    order: VecDeque<u64>,
    max_entries: usize,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl CodesignCache {
    fn new(max_entries: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            max_entries,
        }
    }

    fn get(&self, ino: &u64) -> Option<&Vec<u8>> {
        self.map.get(ino)
    }

    fn insert(&mut self, ino: u64, data: Vec<u8>) {
        // Only track insertion order for genuinely new inodes so re-materializing
        // the same binary doesn't create a duplicate order entry.
        if self.map.insert(ino, data).is_none() {
            self.order.push_back(ino);
            while self.map.len() > self.max_entries {
                match self.order.pop_front() {
                    Some(old) => {
                        self.map.remove(&old);
                    }
                    None => break,
                }
            }
        }
    }
}

/// Read the first `n` bytes of a file (fewer when the file is shorter).
///
/// Used to load just the shebang region during plan construction; `n` comes
/// from the recorded `shebang_length`, so it is bounded by `take` rather than
/// pre-allocated in case the metadata is nonsense.
fn read_leading_bytes(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    File::open(path)?.take(n as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Pre-computed prefix-replacement plan for a file, keyed by inode.
enum ReplacementPlan {
    /// Text file: shebang-aware plan — the shebang region is transformed once
    /// (exactly as the installer does) and the remaining occurrences are body
    /// offsets spliced on read.
    Text(crate::prefix_replacement::TextPlan),
    /// Binary file: c-string groups, each listing prefix offsets followed by
    /// the NUL terminator position.
    Binary(Vec<Vec<usize>>),
}

pub struct VirtualFS {
    metadata: Vec<MetadataNode>,
    mount_point: PathBuf,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    platform: Platform,
    uid: u32,
    gid: u32,
    /// Pre-computed replacement plans for files with prefix placeholders.
    /// Keyed by inode. Populated eagerly at construction from paths.json
    /// offsets or by scanning the source file.
    offset_cache: HashMap<u64, ReplacementPlan>,
    /// Cache for fully materialized + codesigned binary content (macOS only).
    /// Keyed by inode. Only populated for binary-mode prefix files that need
    /// ad-hoc re-signing, since codesign requires the full file.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    codesign_cache: Mutex<CodesignCache>,
}

impl VirtualFS {
    pub fn new(metadata: Vec<MetadataNode>, mount_point: &Path) -> Self {
        Self::with_platform(metadata, mount_point, Platform::current())
    }

    pub(crate) fn with_platform(
        mut metadata: Vec<MetadataNode>,
        mount_point: &Path,
        platform: Platform,
    ) -> Self {
        let target_prefix = mount_point.to_string_lossy();
        let mut offset_cache = HashMap::new();

        // Eagerly compute replacement offsets and text-mode file sizes.
        for i in 0..metadata.len() {
            let Some(file) = metadata[i].as_file() else {
                continue;
            };
            let Some(placeholder) = &file.prefix_placeholder else {
                continue;
            };

            let ino = (i + 1) as u64;
            let old_prefix = placeholder.placeholder.as_bytes();

            // Resolve the on-disk cache path, preferring cache_prefix_path
            // (set for noarch Python files where virtual path differs from cache path).
            let cache_path = {
                let p = (*file.cache_base_path).to_path_buf();
                let prefix = match &file.cache_prefix_path {
                    Some(cp) => cp.as_path(),
                    None => &metadata[file.parent].as_directory().unwrap().prefix_path,
                };
                p.join(prefix).join(&file.file_name)
            };

            // Build the replacement plan. Both modes prefer the offsets
            // recorded in paths.json — that metadata exists precisely so
            // consumers don't have to scan file contents. Per the CEP, rattler
            // applies exactly the groups its own search-based replacement
            // covers (UTF-8 only): `Some(selection)` below is usable metadata
            // (`selection = None` meaning there are validly no UTF-8
            // occurrences to splice), while `None` sends the file down the
            // scanning fallback — the field is absent (pre-CEP package) or
            // structurally invalid/unrecognized. The selected ranges are then
            // trusted as-is (the ranged reads are total, so a non-conformant
            // producer yields wrong bytes for its own package, never a panic).
            let recorded_ranges: Option<Option<&OffsetRanges>> =
                match &placeholder.experimental_offsets {
                    None => None,
                    Some(groups) => match select_utf8_offset_ranges(
                        groups,
                        placeholder.file_mode,
                        placeholder.experimental_shebang_length.is_some(),
                    ) {
                        Ok(selection) => Some(selection),
                        Err(e) => {
                            tracing::warn!(
                                "{}: unusable offset metadata ({e}); falling back to scanning",
                                cache_path.display()
                            );
                            None
                        }
                    },
                };

            let plan = match placeholder.file_mode {
                FileMode::Text => {
                    // With recorded offsets, construction reads at most the
                    // shebang region (`shebang_length` bytes) — the one part
                    // of the transformation a bare offset list can't express.
                    let recorded_plan = if let Some(selection) = recorded_ranges {
                        let body_offsets = match selection {
                            Some(OffsetRanges::Text(offsets)) => offsets.clone(),
                            // Validated by the selection: no UTF-8 occurrences
                            // are recorded outside the shebang region.
                            _ => Vec::new(),
                        };
                        let region = match placeholder.experimental_shebang_length {
                            Some(len) if len > 0 => match read_leading_bytes(&cache_path, len) {
                                Ok(region) => region,
                                Err(e) => {
                                    tracing::warn!(
                                        "failed to read {} for offset computation: {}",
                                        cache_path.display(),
                                        e
                                    );
                                    continue;
                                }
                            },
                            _ => Vec::new(),
                        };
                        let plan = crate::prefix_replacement::TextPlan::from_recorded(
                            &region,
                            body_offsets,
                            &placeholder.placeholder,
                            &target_prefix,
                            &platform,
                        );
                        if plan.is_none() {
                            tracing::warn!(
                                "{}: recorded shebang_length does not match the file \
                                 contents; falling back to scanning",
                                cache_path.display()
                            );
                        }
                        plan
                    } else {
                        None
                    };

                    let (text_plan, source_len) = match recorded_plan {
                        Some(plan) => match fs::symlink_metadata(&cache_path) {
                            Ok(m) => (plan, m.len() as usize),
                            Err(e) => {
                                tracing::warn!(
                                    "failed to stat {} for offset computation: {}",
                                    cache_path.display(),
                                    e
                                );
                                continue;
                            }
                        },
                        None => match fs::read(&cache_path) {
                            Ok(source) => {
                                let plan = crate::prefix_replacement::plan_text_replacement(
                                    &source,
                                    &placeholder.placeholder,
                                    &target_prefix,
                                    &platform,
                                );
                                (plan, source.len())
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "failed to read {} for offset computation: {}",
                                    cache_path.display(),
                                    e
                                );
                                continue;
                            }
                        },
                    };

                    // Post-replacement size: the transformed shebang region plus
                    // the unchanged body length plus the per-occurrence delta.
                    let delta =
                        target_prefix.len() as isize - placeholder.placeholder.len() as isize;
                    let body_len = source_len.saturating_sub(text_plan.region_end);
                    let new_size = (text_plan.transformed_region.len() as isize
                        + body_len as isize
                        + delta * text_plan.body_offsets.len() as isize)
                        .max(0) as u64;
                    metadata[i].as_file_mut().unwrap().computed_size = Some(new_size);

                    ReplacementPlan::Text(text_plan)
                }
                FileMode::Binary => {
                    let groups = match recorded_ranges {
                        Some(Some(OffsetRanges::Binary(g))) => g.clone(),
                        // Valid metadata with no UTF-8 group: nothing to
                        // splice, and empty groups make the ranged reads serve
                        // the bytes verbatim.
                        Some(None) => Vec::new(),
                        _ => match fs::read(&cache_path) {
                            Ok(source) => crate::prefix_replacement::collect_binary_offsets(
                                &source, old_prefix,
                            ),
                            Err(e) => {
                                tracing::warn!(
                                    "failed to read {} for offset computation: {}",
                                    cache_path.display(),
                                    e
                                );
                                continue;
                            }
                        },
                    };
                    ReplacementPlan::Binary(groups)
                }
            };

            offset_cache.insert(ino, plan);
        }

        VirtualFS {
            metadata,
            mount_point: mount_point.to_path_buf(),
            platform,
            uid: current_uid_gid().0,
            gid: current_uid_gid().1,
            offset_cache,
            codesign_cache: Mutex::new(CodesignCache::new(16)),
        }
    }

    /// Validate an inode number and return the 0-based metadata index.
    fn validate_ino(&self, ino: u64) -> Result<usize, i32> {
        if ino == 0 || ino > self.metadata.len() as u64 {
            return Err(ENOENT);
        }
        Ok((ino - 1) as usize)
    }

    /// Build a `FileAttr` with common defaults (uid/gid cached).
    fn make_attr(&self, ino: u64, size: u64, kind: FileKind, perm: u16) -> FileAttr {
        FileAttr {
            ino,
            size,
            blocks: 0,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            kind,
            perm,
            nlink: 1,
            uid: self.uid,
            gid: self.gid,
        }
    }

    fn _getpath(&self, file: &FileNode) -> PathBuf {
        let mut path = (*file.cache_base_path).to_path_buf();
        let prefix = match &file.cache_prefix_path {
            Some(p) => p.as_path(),
            None => {
                &self.metadata[file.parent]
                    .as_directory()
                    .unwrap()
                    .prefix_path
            }
        };
        path = path.join(prefix);
        path.join(&file.file_name)
    }

    fn _getattr(&self, child: &MetadataNode, child_index: &usize) -> FileAttr {
        let ino = (child_index + 1) as u64;
        match child {
            MetadataNode::Directory(_) => self.make_attr(ino, 0, FileKind::Directory, 0o755),
            MetadataNode::File(file) => {
                if let Some(ref content) = file.virtual_content {
                    return self.make_attr(ino, content.len() as u64, FileKind::RegularFile, 0o775);
                }

                let path = self._getpath(file);
                match fs::symlink_metadata(&path) {
                    Ok(metadata) => {
                        let mut attr = FileAttr::from_metadata(&metadata, ino);
                        // Override size if prefix replacement changes the file length
                        if let Some(computed) = file.computed_size {
                            attr.size = computed;
                        }
                        attr
                    }
                    Err(e) => {
                        tracing::warn!("failed to stat {}: {}", path.display(), e);
                        self.make_attr(ino, 0, FileKind::RegularFile, 0o644)
                    }
                }
            }
        }
    }

    // -- Testable inner methods --

    pub(crate) fn do_lookup(&self, parent_ino: u64, name: &OsStr) -> Result<FileAttr, i32> {
        let parent_index = self.validate_ino(parent_ino)?;

        let Some(parent_directory) = self.metadata[parent_index].as_directory() else {
            return Err(ENOTDIR);
        };

        for child_index in parent_directory.children.iter() {
            let child = &self.metadata[*child_index];
            if names_match(child.file_name(), name) {
                return Ok(self._getattr(child, child_index));
            }
        }

        Err(ENOENT)
    }

    pub(crate) fn do_getattr(&self, ino: u64) -> Result<FileAttr, i32> {
        let index = self.validate_ino(ino)?;
        let entry = &self.metadata[index];
        Ok(self._getattr(entry, &index))
    }

    pub(crate) fn do_readlink(&self, ino: u64) -> Result<PathBuf, i32> {
        let index = self.validate_ino(ino)?;
        let Some(current_file) = self.metadata[index].as_file() else {
            return Err(ENOENT);
        };
        let path = self._getpath(current_file);
        fs::read_link(&path).map_err(|e| {
            tracing::warn!("readlink failed for {}: {}", path.display(), e);
            EIO
        })
    }

    pub(crate) fn do_content_source(&self, ino: u64) -> Result<ContentSource, i32> {
        let index = self.validate_ino(ino)?;

        let Some(current_file) = self.metadata[index].as_file() else {
            return Err(ENOENT); // directories don't have readable content
        };

        if current_file.path_type == PathType::SoftLink {
            return Err(ENOENT); // symlinks don't have readable content
        }

        if current_file.virtual_content.is_some() {
            return Ok(ContentSource::Virtual);
        }

        if current_file.prefix_placeholder.is_some() {
            return Ok(ContentSource::Transformed);
        }

        let path = self._getpath(current_file);
        Ok(ContentSource::Direct(path))
    }

    pub(crate) fn do_read(&self, ino: u64, offset: u64, size: u32) -> Result<Vec<u8>, i32> {
        let index = self.validate_ino(ino)?;

        let Some(current_file) = self.metadata[index].as_file() else {
            return Ok(vec![]); // directories
        };

        if current_file.path_type == PathType::SoftLink {
            return Ok(vec![]); // symlinks
        }

        // Virtual files (e.g. entry points) are served directly from memory
        if let Some(ref content) = current_file.virtual_content {
            let start = (offset as usize).min(content.len());
            let end = (start + size as usize).min(content.len());
            return Ok(content[start..end].to_vec());
        }

        let path = self._getpath(current_file);

        // Files without prefix replacement: read directly from disk
        if current_file.prefix_placeholder.is_none() {
            let mut file = File::open(&path).map_err(|e| {
                tracing::warn!("failed to open {}: {}", path.display(), e);
                EIO
            })?;
            file.seek(SeekFrom::Start(offset)).map_err(|e| {
                tracing::warn!("failed to seek {}: {}", path.display(), e);
                EIO
            })?;
            let mut buf = vec![0u8; size as usize];
            let n = file.read(&mut buf).map_err(|e| {
                tracing::warn!("failed to read {}: {}", path.display(), e);
                EIO
            })?;
            buf.truncate(n);
            return Ok(buf);
        }

        // Has prefix placeholder — use ranged replacement
        let placeholder = current_file.prefix_placeholder.as_ref().unwrap();

        let file = File::open(&path).map_err(|e| {
            tracing::warn!("failed to open {}: {}", path.display(), e);
            EIO
        })?;

        let mmap = unsafe { Mmap::map(&file) }.map_err(|e| {
            tracing::warn!("failed to memory map {}: {}", path.display(), e);
            EIO
        })?;

        let old_prefix = placeholder.placeholder.as_bytes();
        let new_prefix_str = self.mount_point.to_string_lossy();
        let new_prefix = new_prefix_str.as_bytes();

        let start = offset as usize;
        let end = start + size as usize;

        let Some(plan) = self.offset_cache.get(&ino) else {
            // No plan — serve source bytes directly
            let s = start.min(mmap.len());
            let e = (s + size as usize).min(mmap.len());
            return Ok(mmap[s..e].to_vec());
        };

        match plan {
            ReplacementPlan::Binary(groups) => {
                // macOS binaries need codesign after prefix replacement.
                // Codesign rehashes every page so it can't be done as a ranged
                // operation. Materialize + resign once, cache for subsequent reads.
                // The codesign module is compiled only on macOS — other targets
                // fall through to `binary_ranged_read` directly. With no
                // occurrences to replace the bytes are served verbatim and the
                // original signature stays valid, so the codesign path is
                // skipped too.
                #[cfg(target_os = "macos")]
                if self.platform.is_osx() && !groups.is_empty() {
                    // Fast path: serve from cache
                    if let Some(cached) = self.codesign_cache.lock().unwrap().get(&ino) {
                        let s = start.min(cached.len());
                        let e = (s + size as usize).min(cached.len());
                        return Ok(cached[s..e].to_vec());
                    }

                    // Slow path: materialize, resign, cache. The plan already
                    // holds the selected (or scanned) UTF-8 c-string groups, so
                    // hand the dispatcher a synthesized UTF-8 offset group.
                    let target_prefix = self.mount_point.to_string_lossy();
                    let mut output = Vec::with_capacity(mmap.len());
                    let offset_groups = [OffsetGroup {
                        encoding: OffsetEncoding::Utf8,
                        ranges: OffsetRanges::Binary(groups.clone()),
                        has_unknown_members: false,
                    }];

                    let result = copy_and_replace_placeholders_with_offsets(
                        &mmap,
                        &mut output,
                        &placeholder.placeholder,
                        &target_prefix,
                        &self.platform,
                        placeholder.file_mode,
                        &offset_groups,
                        placeholder.experimental_shebang_length,
                    );

                    if let Err(e) = result {
                        tracing::warn!(
                            "prefix replacement failed for {} ({}); serving raw bytes",
                            path.display(),
                            e
                        );
                        let s = start.min(mmap.len());
                        let e = (s + size as usize).min(mmap.len());
                        return Ok(mmap[s..e].to_vec());
                    }

                    if let Err(e) = crate::codesign::adhoc_resign(&mut output) {
                        tracing::warn!("ad-hoc re-signing failed for {}: {}", path.display(), e);
                    }

                    let s = start.min(output.len());
                    let e = (s + size as usize).min(output.len());
                    let result = output[s..e].to_vec();
                    self.codesign_cache.lock().unwrap().insert(ino, output);
                    return Ok(result);
                }

                Ok(crate::prefix_replacement::binary_ranged_read(
                    &mmap, old_prefix, new_prefix, groups, start, end,
                ))
            }
            ReplacementPlan::Text(text_plan) => Ok(crate::prefix_replacement::text_ranged_read(
                &mmap,
                old_prefix,
                new_prefix,
                &text_plan.body_offsets,
                text_plan.region_end,
                &text_plan.transformed_region,
                start,
                end,
            )),
        }
    }

    pub(crate) fn do_readdir(&self, ino: u64, offset: u64) -> Result<Vec<DirEntry>, i32> {
        let index = self.validate_ino(ino)?;

        let Some(current_directory) = self.metadata[index].as_directory() else {
            return Err(ENOTDIR);
        };

        let mut entries = Vec::new();

        if offset == 0 {
            entries.push(DirEntry {
                ino: (current_directory.parent + 1) as u64,
                kind: FileKind::Directory,
                name: OsString::from(".."),
            });
        }
        if offset <= 1 {
            entries.push(DirEntry {
                ino,
                kind: FileKind::Directory,
                name: OsString::from("."),
            });
        }

        for child_index in current_directory
            .children
            .iter()
            .skip(offset.saturating_sub(2) as usize)
        {
            let child = &self.metadata[*child_index];
            let kind = match child {
                MetadataNode::Directory(_) => FileKind::Directory,
                MetadataNode::File(f) => {
                    if f.path_type == PathType::SoftLink {
                        FileKind::Symlink
                    } else {
                        FileKind::RegularFile
                    }
                }
            };
            entries.push(DirEntry {
                ino: (child_index + 1) as u64,
                kind,
                name: child.file_name().to_owned(),
            });
        }

        Ok(entries)
    }
}

impl VfsOps for VirtualFS {
    fn lookup(&self, parent: u64, name: &OsStr) -> Result<FileAttr, i32> {
        self.do_lookup(parent, name)
    }
    fn getattr(&self, ino: u64) -> Result<FileAttr, i32> {
        self.do_getattr(ino)
    }
    fn readlink(&self, ino: u64) -> Result<PathBuf, i32> {
        self.do_readlink(ino)
    }
    fn read(&self, ino: u64, offset: u64, size: u32) -> Result<Vec<u8>, i32> {
        self.do_read(ino, offset, size)
    }
    fn content_source(&self, ino: u64) -> Result<ContentSource, i32> {
        self.do_content_source(ino)
    }
    fn readdir(&self, ino: u64, offset: u64) -> Result<Vec<DirEntry>, i32> {
        self.do_readdir(ino, offset)
    }

    fn ino_to_path(&self, ino: u64) -> Result<PathBuf, i32> {
        let index = self.validate_ino(ino)?;
        let entry = &self.metadata[index];

        // Root directory → empty path
        if index == 0 {
            return Ok(PathBuf::new());
        }

        match entry {
            MetadataNode::Directory(dir) => {
                // prefix_path is like "./lib/python3.14" — strip the "./" prefix
                let p = dir
                    .prefix_path
                    .strip_prefix("./")
                    .unwrap_or(&dir.prefix_path);
                Ok(p.to_path_buf())
            }
            MetadataNode::File(file) => {
                let parent = &self.metadata[file.parent];
                let parent_dir = parent.as_directory().ok_or(ENOENT)?;
                let parent_path = parent_dir
                    .prefix_path
                    .strip_prefix("./")
                    .unwrap_or(&parent_dir.prefix_path);
                Ok(parent_path.join(&file.file_name))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{new_empty_tree, path_parse};

    #[test]
    fn test_codesign_cache_bounds_and_evicts_oldest() {
        let mut cache = CodesignCache::new(2);
        cache.insert(1, vec![1]);
        cache.insert(2, vec![2]);
        assert!(cache.get(&1).is_some());
        assert!(cache.get(&2).is_some());

        // Inserting a third entry evicts the oldest (inode 1).
        cache.insert(3, vec![3]);
        assert!(cache.get(&1).is_none());
        assert!(cache.get(&2).is_some());
        assert!(cache.get(&3).is_some());

        // Re-inserting an existing inode updates in place without growing the
        // cache or evicting a live entry.
        cache.insert(2, vec![22]);
        assert_eq!(cache.get(&2), Some(&vec![22]));
        assert!(cache.get(&3).is_some());
    }
    use rattler_conda_types::package::{
        FileMode, PathType, PathsEntry, PathsJson, PrefixPlaceholder,
    };
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file as symlink;
    use tempfile::TempDir;

    /// Build a test fixture:
    /// ```text
    /// tmpdir/
    /// ├── lib/
    /// │   ├── libfoo.so          "hello world"
    /// │   ├── libfoo.so.1 → libfoo.so
    /// │   └── libbar.so → gone.so   (dangling)
    /// ├── etc/
    /// │   └── config.txt         "/old/prefix/path/to/thing"
    /// └── bin/
    ///     └── run.sh             "#!/old/prefix/bin/python\nprint('hi')"
    /// ```
    fn create_fixture() -> (TempDir, VirtualFS) {
        let tmpdir = TempDir::new().unwrap();
        let cache_path = tmpdir.path();

        // Create directories
        fs::create_dir_all(cache_path.join("lib")).unwrap();
        fs::create_dir_all(cache_path.join("etc")).unwrap();
        fs::create_dir_all(cache_path.join("bin")).unwrap();

        // Create files
        fs::write(cache_path.join("lib/libfoo.so"), b"hello world").unwrap();
        symlink("libfoo.so", cache_path.join("lib/libfoo.so.1")).unwrap();
        symlink("gone.so", cache_path.join("lib/libbar.so")).unwrap();
        fs::write(
            cache_path.join("etc/config.txt"),
            b"/old/prefix/path/to/thing",
        )
        .unwrap();
        fs::write(
            cache_path.join("bin/run.sh"),
            b"#!/old/prefix/bin/python\nprint('hi')",
        )
        .unwrap();

        // Build PathsJson
        let paths_json = PathsJson {
            paths: vec![
                PathsEntry {
                    relative_path: PathBuf::from("lib/libfoo.so"),
                    path_type: PathType::HardLink,
                    prefix_placeholder: None,
                    no_link: false,
                    sha256: None,
                    size_in_bytes: None,
                },
                PathsEntry {
                    relative_path: PathBuf::from("lib/libfoo.so.1"),
                    path_type: PathType::SoftLink,
                    prefix_placeholder: None,
                    no_link: false,
                    sha256: None,
                    size_in_bytes: None,
                },
                PathsEntry {
                    relative_path: PathBuf::from("lib/libbar.so"),
                    path_type: PathType::SoftLink,
                    prefix_placeholder: None,
                    no_link: false,
                    sha256: None,
                    size_in_bytes: None,
                },
                PathsEntry {
                    relative_path: PathBuf::from("etc/config.txt"),
                    path_type: PathType::HardLink,
                    prefix_placeholder: Some(PrefixPlaceholder {
                        file_mode: FileMode::Text,
                        placeholder: "/old/prefix".to_string(),
                        experimental_offsets: None,
                        experimental_shebang_length: None,
                    }),
                    no_link: false,
                    sha256: None,
                    size_in_bytes: None,
                },
                PathsEntry {
                    relative_path: PathBuf::from("bin/run.sh"),
                    path_type: PathType::HardLink,
                    prefix_placeholder: Some(PrefixPlaceholder {
                        file_mode: FileMode::Text,
                        placeholder: "/old/prefix".to_string(),
                        experimental_offsets: None,
                        experimental_shebang_length: None,
                    }),
                    no_link: false,
                    sha256: None,
                    size_in_bytes: None,
                },
            ],
            paths_version: 1,
        };

        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            cache_path,
            None,
            &mut env_paths,
            &mut dir_indices,
        );

        let mount_point = PathBuf::from("/new/prefix");
        let vfs = VirtualFS::with_platform(env_paths, &mount_point, Platform::Linux64);

        (tmpdir, vfs)
    }

    // --- lookup tests ---

    #[test]
    fn test_lookup_directory() {
        let (_tmp, vfs) = create_fixture();
        let attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        assert_eq!(attr.kind, FileKind::Directory);
    }

    #[test]
    fn test_lookup_file() {
        let (_tmp, vfs) = create_fixture();
        // First find lib directory
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let lib_ino = lib_attr.ino;
        // Then find file in lib
        let attr = vfs.do_lookup(lib_ino, OsStr::new("libfoo.so")).unwrap();
        assert_eq!(attr.kind, FileKind::RegularFile);
        assert!(attr.size > 0);
    }

    #[test]
    fn test_lookup_not_found() {
        let (_tmp, vfs) = create_fixture();
        assert_eq!(
            vfs.do_lookup(1, OsStr::new("nonexistent")).unwrap_err(),
            ENOENT
        );
    }

    #[test]
    fn test_lookup_not_directory() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let file_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libfoo.so"))
            .unwrap();
        // Try to lookup child of a file
        assert_eq!(
            vfs.do_lookup(file_attr.ino, OsStr::new("child"))
                .unwrap_err(),
            ENOTDIR
        );
    }

    // --- getattr tests ---

    #[test]
    fn test_getattr_root() {
        let (_tmp, vfs) = create_fixture();
        let attr = vfs.do_getattr(1).unwrap();
        assert_eq!(attr.kind, FileKind::Directory);
    }

    #[test]
    fn test_getattr_regular_file() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let file_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libfoo.so"))
            .unwrap();
        let attr = vfs.do_getattr(file_attr.ino).unwrap();
        assert_eq!(attr.kind, FileKind::RegularFile);
        assert_eq!(attr.size, 11); // "hello world"
    }

    #[test]
    fn test_getattr_symlink() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let sym_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libfoo.so.1"))
            .unwrap();
        assert_eq!(sym_attr.kind, FileKind::Symlink);
    }

    #[test]
    fn test_getattr_dangling_symlink() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let sym_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libbar.so"))
            .unwrap();
        // dangling symlink still reports as symlink via symlink_metadata
        assert_eq!(sym_attr.kind, FileKind::Symlink);
    }

    #[test]
    fn test_getattr_invalid_ino() {
        let (_tmp, vfs) = create_fixture();
        assert_eq!(vfs.do_getattr(9999).unwrap_err(), ENOENT);
    }

    // --- readlink tests ---

    #[test]
    fn test_readlink_valid() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let sym_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libfoo.so.1"))
            .unwrap();
        let target = vfs.do_readlink(sym_attr.ino).unwrap();
        assert_eq!(target, PathBuf::from("libfoo.so"));
    }

    #[test]
    fn test_readlink_dangling() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let sym_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libbar.so"))
            .unwrap();
        let target = vfs.do_readlink(sym_attr.ino).unwrap();
        assert_eq!(target, PathBuf::from("gone.so"));
    }

    #[test]
    fn test_readlink_regular_file() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let file_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libfoo.so"))
            .unwrap();
        // read_link on a regular file should fail
        assert_eq!(vfs.do_readlink(file_attr.ino), Err(EIO));
    }

    #[test]
    fn test_readlink_directory() {
        let (_tmp, vfs) = create_fixture();
        // readlink on a directory should fail (not a file)
        assert_eq!(vfs.do_readlink(1), Err(ENOENT));
    }

    // --- read tests ---

    #[test]
    fn test_read_regular_file() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let file_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libfoo.so"))
            .unwrap();
        let data = vfs.do_read(file_attr.ino, 0, 1024).unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn test_read_with_offset() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let file_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libfoo.so"))
            .unwrap();
        let data = vfs.do_read(file_attr.ino, 6, 5).unwrap();
        assert_eq!(data, b"world");
    }

    #[test]
    fn test_read_with_prefix_replacement() {
        let (_tmp, vfs) = create_fixture();
        let etc_attr = vfs.do_lookup(1, OsStr::new("etc")).unwrap();
        let config_attr = vfs
            .do_lookup(etc_attr.ino, OsStr::new("config.txt"))
            .unwrap();
        let data = vfs.do_read(config_attr.ino, 0, 4096).unwrap();
        let content = String::from_utf8(data).unwrap();
        assert!(
            content.contains("/new/prefix"),
            "expected /new/prefix in: {content}"
        );
        assert!(
            !content.contains("/old/prefix"),
            "unexpected /old/prefix in: {content}"
        );
    }

    #[test]
    fn test_getattr_text_prefix_reports_correct_size() {
        let (_tmp, vfs) = create_fixture();
        let etc_attr = vfs.do_lookup(1, OsStr::new("etc")).unwrap();
        let config_attr = vfs
            .do_lookup(etc_attr.ino, OsStr::new("config.txt"))
            .unwrap();

        // getattr size should match actual content length, not on-disk size
        let data = vfs.do_read(config_attr.ino, 0, u32::MAX).unwrap();
        assert_eq!(
            config_attr.size,
            data.len() as u64,
            "getattr size ({}) != read content length ({})",
            config_attr.size,
            data.len()
        );
    }

    #[test]
    fn test_getattr_shebang_prefix_reports_correct_size() {
        let (_tmp, vfs) = create_fixture();
        let bin_attr = vfs.do_lookup(1, OsStr::new("bin")).unwrap();
        let run_attr = vfs.do_lookup(bin_attr.ino, OsStr::new("run.sh")).unwrap();

        // Shebang file: getattr size should match actual content length
        let data = vfs.do_read(run_attr.ino, 0, u32::MAX).unwrap();
        assert_eq!(
            run_attr.size,
            data.len() as u64,
            "getattr size ({}) != read content length ({})",
            run_attr.size,
            data.len()
        );
    }

    #[test]
    fn test_read_directory_returns_empty() {
        let (_tmp, vfs) = create_fixture();
        let data = vfs.do_read(1, 0, 1024).unwrap(); // root directory
        assert!(data.is_empty());
    }

    #[test]
    fn test_read_symlink_returns_empty() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let sym_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libfoo.so.1"))
            .unwrap();
        let data = vfs.do_read(sym_attr.ino, 0, 1024).unwrap();
        assert!(data.is_empty());
    }

    #[test]
    fn test_read_dangling_symlink_returns_empty() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let sym_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libbar.so"))
            .unwrap();
        let data = vfs.do_read(sym_attr.ino, 0, 1024).unwrap();
        assert!(data.is_empty());
    }

    // --- readdir tests ---

    #[test]
    fn test_readdir_root() {
        let (_tmp, vfs) = create_fixture();
        let entries = vfs.do_readdir(1, 0).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
        assert!(names.contains(&".."));
        assert!(names.contains(&"."));
        assert!(names.contains(&"lib"));
        assert!(names.contains(&"etc"));
        assert!(names.contains(&"bin"));
    }

    #[test]
    fn test_readdir_subdirectory() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let entries = vfs.do_readdir(lib_attr.ino, 0).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
        assert!(names.contains(&"libfoo.so"));
        assert!(names.contains(&"libfoo.so.1"));
        assert!(names.contains(&"libbar.so"));
    }

    #[test]
    fn test_readdir_reports_symlink_kind() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let entries = vfs.do_readdir(lib_attr.ino, 0).unwrap();

        let find = |name: &str| {
            entries
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("entry {name} not found"))
        };
        assert_eq!(find("libfoo.so").kind, FileKind::RegularFile);
        assert_eq!(find("libfoo.so.1").kind, FileKind::Symlink);
        assert_eq!(find("libbar.so").kind, FileKind::Symlink);
    }

    #[test]
    fn test_readdir_with_offset() {
        let (_tmp, vfs) = create_fixture();
        let all = vfs.do_readdir(1, 0).unwrap();
        let skipped = vfs.do_readdir(1, 3).unwrap();
        assert!(skipped.len() < all.len());
    }

    #[test]
    fn test_readdir_not_directory() {
        let (_tmp, vfs) = create_fixture();
        let lib_attr = vfs.do_lookup(1, OsStr::new("lib")).unwrap();
        let file_attr = vfs
            .do_lookup(lib_attr.ino, OsStr::new("libfoo.so"))
            .unwrap();
        assert_eq!(vfs.do_readdir(file_attr.ino, 0), Err(ENOTDIR));
    }

    // --- virtual file tests ---

    fn create_fixture_with_virtual_files() -> (TempDir, VirtualFS) {
        use rattler::install::PythonInfo;
        use rattler_conda_types::Version;
        use std::str::FromStr;

        let tmpdir = TempDir::new().unwrap();
        let cache_path = tmpdir.path();

        fs::create_dir_all(cache_path.join("bin")).unwrap();
        fs::write(cache_path.join("bin/real_tool"), b"real content").unwrap();

        let paths_json = PathsJson {
            paths: vec![PathsEntry {
                relative_path: PathBuf::from("bin/real_tool"),
                path_type: PathType::HardLink,
                prefix_placeholder: None,
                no_link: false,
                sha256: None,
                size_in_bytes: None,
            }],
            paths_version: 1,
        };

        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            cache_path,
            None,
            &mut env_paths,
            &mut dir_indices,
        );

        // Add a virtual entry point
        let python_info = PythonInfo::from_version(
            &Version::from_str("3.11.0").unwrap(),
            None,
            Platform::Linux64,
        )
        .unwrap();
        let ep = rattler_conda_types::package::EntryPoint::from_str("mytool = mymod:main").unwrap();
        crate::add_entry_points(
            &[ep],
            "/new/prefix",
            &python_info,
            &mut env_paths,
            &mut dir_indices,
        );

        let vfs = VirtualFS::with_platform(env_paths, Path::new("/new/prefix"), Platform::Linux64);
        (tmpdir, vfs)
    }

    #[test]
    fn test_lookup_entry_point() {
        let (_tmp, vfs) = create_fixture_with_virtual_files();
        let bin_attr = vfs.do_lookup(1, OsStr::new("bin")).unwrap();
        let ep_attr = vfs.do_lookup(bin_attr.ino, OsStr::new("mytool")).unwrap();
        assert_eq!(ep_attr.kind, FileKind::RegularFile);
        assert!(ep_attr.size > 0);
    }

    #[test]
    fn test_getattr_virtual_file() {
        let (_tmp, vfs) = create_fixture_with_virtual_files();
        let bin_attr = vfs.do_lookup(1, OsStr::new("bin")).unwrap();
        let ep_attr = vfs.do_lookup(bin_attr.ino, OsStr::new("mytool")).unwrap();
        assert_eq!(ep_attr.kind, FileKind::RegularFile);
        assert_eq!(ep_attr.perm, 0o775); // executable
        assert!(ep_attr.size > 0);
    }

    /// Noarch Python packages store scripts under `python-scripts/` on disk
    /// but expose them as `bin/` in the virtual tree. When paths.json has no
    /// precomputed offsets, the VFS must resolve the *cache* path (via
    /// `cache_prefix_path`) instead of the virtual parent directory path.
    /// Regression test for prefix-replacement warnings on noarch entry-point
    /// scripts (e.g. "failed to read …/bin/script for offset computation").
    #[test]
    fn test_noarch_prefix_replacement_uses_cache_path() {
        use rattler::install::PythonInfo;
        use rattler_conda_types::Version;
        use std::str::FromStr;

        let tmpdir = TempDir::new().unwrap();
        let cache_path = tmpdir.path();

        // On disk the file lives under python-scripts/ (the raw package layout)
        fs::create_dir_all(cache_path.join("python-scripts")).unwrap();
        let script_content = b"#!/old/prefix/bin/python\nprint('hello')";
        fs::write(cache_path.join("python-scripts/myscript"), script_content).unwrap();

        // Build PathsJson with a prefix placeholder but NO precomputed offsets
        let paths_json = PathsJson {
            paths: vec![PathsEntry {
                relative_path: PathBuf::from("python-scripts/myscript"),
                path_type: PathType::HardLink,
                prefix_placeholder: Some(PrefixPlaceholder {
                    file_mode: FileMode::Text,
                    placeholder: "/old/prefix".to_string(),
                    experimental_offsets: None,
                    experimental_shebang_length: None,
                }),
                no_link: false,
                sha256: None,
                size_in_bytes: None,
            }],
            paths_version: 1,
        };

        let python_info = PythonInfo::from_version(
            &Version::from_str("3.11.0").unwrap(),
            None,
            Platform::Linux64,
        )
        .unwrap();

        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            cache_path,
            Some(&python_info),
            &mut env_paths,
            &mut dir_indices,
        );

        let mount_point = PathBuf::from("/new/prefix");
        let vfs = VirtualFS::with_platform(env_paths, &mount_point, Platform::Linux64);

        // The file should appear under bin/ in the virtual tree
        let bin_attr = vfs.do_lookup(1, OsStr::new("bin")).unwrap();
        let script_attr = vfs.do_lookup(bin_attr.ino, OsStr::new("myscript")).unwrap();

        // Read should perform prefix replacement successfully
        let data = vfs.do_read(script_attr.ino, 0, u32::MAX).unwrap();
        let content = String::from_utf8(data.clone()).unwrap();
        assert!(
            content.contains("/new/prefix"),
            "expected /new/prefix in: {content}"
        );
        assert!(
            !content.contains("/old/prefix"),
            "unexpected /old/prefix in: {content}"
        );

        // getattr size should match actual read content length
        assert_eq!(
            script_attr.size,
            data.len() as u64,
            "getattr size ({}) != read content length ({})",
            script_attr.size,
            data.len()
        );
    }

    #[test]
    fn test_read_virtual_file() {
        let (_tmp, vfs) = create_fixture_with_virtual_files();
        let bin_attr = vfs.do_lookup(1, OsStr::new("bin")).unwrap();
        let ep_attr = vfs.do_lookup(bin_attr.ino, OsStr::new("mytool")).unwrap();
        let data = vfs.do_read(ep_attr.ino, 0, u32::MAX).unwrap();
        assert!(!data.is_empty());

        let content = String::from_utf8(data).unwrap();
        assert!(
            content.contains("#!/new/prefix/bin/python3.11"),
            "shebang missing: {content}"
        );
        assert!(
            content.contains("from mymod import"),
            "import missing: {content}"
        );
        assert!(
            content.contains("main()"),
            "function call missing: {content}"
        );
    }
}
