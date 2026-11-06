//! Per-package index of everything a mount needs to know about a package.
//!
//! Mounting an environment needs, for every package: the list of its files
//! (`info/paths.json`), each file's attributes, where the prefix placeholder
//! occurs in the files that have one, and for noarch Python packages their
//! entry points (`info/link.json`). Gathering that from scratch means parsing
//! JSON, `stat`ing every file and reading every prefix-bearing file in full
//! (most packages don't record placeholder offsets in `paths.json` yet; a
//! Python environment has tens of megabytes of such binaries).
//!
//! None of that depends on the environment the package is mounted in, so it is
//! computed once per package and stored next to the extracted package as an
//! [rkyv] archive ([`INDEX_PATH`]). Later mounts memory-map the archive and
//! read it in place: no parsing, no per-file `stat`, no scanning. Because the
//! index lives inside the package directory, it disappears together with the
//! package when the cache re-extracts or removes it.
//!
//! The index is an optimization only. A missing, outdated, corrupt or
//! unwritable index (e.g. in a read-only cache layer) is rebuilt in memory, and
//! failing to store it is logged and otherwise ignored.

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use memmap2::Mmap;
use rattler::install::link::CStringOccurrences;
use rattler_conda_types::{
    Subdir,
    package::{
        FileMode, LinkJson, NoArchLinks, OffsetEncoding, OffsetRanges, PackageFile, PathType,
        PathsEntry, PathsJson, PrefixOffsets,
    },
};
use rayon::prelude::*;
use rkyv::{Archive, Deserialize, Serialize, rancor, util::AlignedVec};

use crate::vfs_ops::FileKind;

/// Where the index is stored, relative to the package directory.
pub(crate) const INDEX_PATH: &str = "info/rattler-vfs-index.rkyv";

/// Bump whenever the layout or the meaning of the stored data changes.
const VERSION: u32 = 2;

/// Bytes before the archive in [`INDEX_PATH`]: a magic number and the
/// xxh3 hash of the archive. Sixteen bytes keep the archive aligned in the
/// (page-aligned) mapping.
const HEADER_LEN: usize = 16;
const MAGIC: &[u8; 8] = b"rvfsidx\0";

// ---------------------------------------------------------------------------
// Stored format
// ---------------------------------------------------------------------------

/// The index of one extracted package.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct PackageIndex {
    version: u32,
    /// The target family the scans were made for (see [`target_family`]):
    /// binaries are never patched on Windows, and shebangs are only rewritten
    /// on Unix.
    target_family: u8,
    /// Identity of the package's file list when the index was built.
    paths_json: Stamp,
    /// Entry points from `info/link.json` (noarch Python packages only).
    pub(crate) entry_points: Vec<IndexEntryPoint>,
    /// One entry per `paths.json` entry, in the same order.
    pub(crate) files: Vec<IndexFile>,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexEntryPoint {
    pub(crate) command: String,
    pub(crate) module: String,
    pub(crate) function: String,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct IndexFile {
    /// The `/`-separated path relative to the package root.
    pub(crate) path: String,
    path_type: u8,
    /// `None` when the file could not be `stat`ed while indexing.
    stat: Option<IndexStat>,
    placeholder: Option<IndexPlaceholder>,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct IndexStat {
    len: u64,
    blocks: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
    ctime_secs: u64,
    ctime_nanos: u32,
    perm: u16,
    uid: u32,
    gid: u32,
    kind: u8,
    /// `(flags, source mtime, source size)` from the header of a `.pyc` file.
    pyc_header: Option<(u32, u32, u32)>,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct IndexPlaceholder {
    placeholder: String,
    text: bool,
    /// `None` when the file could not be read while indexing.
    scan: Option<IndexScan>,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
enum IndexScan {
    Text {
        region_end: u64,
        /// `(offset, encoding)`, encodings as indices into
        /// [`OffsetEncoding::DEFINED`].
        occurrences: Vec<(u64, u8)>,
    },
    Binary {
        /// `(encoding, offsets, nul_pos)` per c-string.
        cstrings: Vec<(u8, Vec<u64>, u64)>,
    },
}

// ---------------------------------------------------------------------------
// In-memory types handed to the metadata tree
// ---------------------------------------------------------------------------

/// The attributes of a package file, captured when the package was indexed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileStat {
    pub(crate) len: u64,
    pub(crate) blocks: u64,
    pub(crate) mtime: SystemTime,
    pub(crate) ctime: SystemTime,
    pub(crate) perm: u16,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) kind: FileKind,
    /// `(flags, source mtime, source size)` of a `.pyc` file's header.
    pub(crate) pyc_header: Option<(u32, u32, u32)>,
}

/// Where the placeholder occurs in one file, independent of the target prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScanResult {
    /// See [`crate::prefix_replacement::scan_text`].
    Text {
        region_end: usize,
        occurrences: Vec<(usize, OffsetEncoding)>,
    },
    /// The c-strings to patch, see
    /// [`crate::prefix_replacement::plan_binary_replacement`].
    Binary(Vec<CStringOccurrences>),
}

/// A view of one file of an archived index.
pub(crate) struct FileRef<'a> {
    file: &'a ArchivedIndexFile,
}

impl<'a> FileRef<'a> {
    pub(crate) fn path(&self) -> &'a str {
        self.file.path.as_str()
    }

    pub(crate) fn path_type(&self) -> PathType {
        match self.file.path_type {
            1 => PathType::SoftLink,
            2 => PathType::Directory,
            _ => PathType::HardLink,
        }
    }

    pub(crate) fn stat(&self) -> Option<FileStat> {
        let stat = self.file.stat.as_ref()?;
        let time = |secs: u64, nanos: u32| UNIX_EPOCH + Duration::new(secs, nanos);
        Some(FileStat {
            len: stat.len.to_native(),
            blocks: stat.blocks.to_native(),
            mtime: time(stat.mtime_secs.to_native(), stat.mtime_nanos.to_native()),
            ctime: time(stat.ctime_secs.to_native(), stat.ctime_nanos.to_native()),
            perm: stat.perm.to_native(),
            uid: stat.uid.to_native(),
            gid: stat.gid.to_native(),
            kind: kind_from_u8(stat.kind),
            pyc_header: stat
                .pyc_header
                .as_ref()
                .map(|h| (h.0.to_native(), h.1.to_native(), h.2.to_native())),
        })
    }

    /// The placeholder (string and file mode) and, when it could be computed,
    /// where it occurs.
    pub(crate) fn placeholder(&self) -> Option<(&'a str, FileMode, Option<ScanResult>)> {
        let placeholder = self.file.placeholder.as_ref()?;
        let mode = if placeholder.text {
            FileMode::Text
        } else {
            FileMode::Binary
        };
        let scan = placeholder.scan.as_ref().and_then(|scan| {
            Some(match scan {
                ArchivedIndexScan::Text {
                    region_end,
                    occurrences,
                } => ScanResult::Text {
                    region_end: region_end.to_native() as usize,
                    occurrences: occurrences
                        .iter()
                        .map(|o| Some((o.0.to_native() as usize, encoding_from_u8(o.1)?)))
                        .collect::<Option<_>>()?,
                },
                ArchivedIndexScan::Binary { cstrings } => ScanResult::Binary(
                    cstrings
                        .iter()
                        .map(|c| {
                            Some(CStringOccurrences {
                                encoding: encoding_from_u8(c.0)?,
                                offsets: c.1.iter().map(|o| o.to_native() as usize).collect(),
                                nul_pos: c.2.to_native() as usize,
                            })
                        })
                        .collect::<Option<_>>()?,
                ),
            })
        });
        Some((placeholder.placeholder.as_str(), mode, scan))
    }
}

fn kind_to_u8(kind: FileKind) -> u8 {
    match kind {
        FileKind::RegularFile => 0,
        FileKind::Symlink => 1,
        FileKind::Directory => 2,
    }
}

fn kind_from_u8(kind: u8) -> FileKind {
    match kind {
        1 => FileKind::Symlink,
        2 => FileKind::Directory,
        _ => FileKind::RegularFile,
    }
}

fn encoding_to_u8(encoding: OffsetEncoding) -> u8 {
    OffsetEncoding::DEFINED
        .iter()
        .position(|&e| e == encoding)
        .expect("every encoding is defined") as u8
}

fn encoding_from_u8(index: u8) -> Option<OffsetEncoding> {
    OffsetEncoding::DEFINED.get(index as usize).copied()
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// The bytes of a validated index archive.
pub(crate) enum IndexBytes {
    /// Memory-mapped from [`INDEX_PATH`], including its header.
    Mapped(Mmap),
    /// Built during this mount.
    Owned(AlignedVec),
}

impl IndexBytes {
    pub(crate) fn archived(&self) -> &ArchivedPackageIndex {
        let bytes: &[u8] = match self {
            Self::Mapped(mmap) => &mmap[HEADER_LEN..],
            Self::Owned(bytes) => bytes,
        };
        // SAFETY: both variants are only constructed from bytes that passed
        // `rkyv::access` validation (see `validate`), and neither the mapping
        // (the file is replaced by rename, never modified in place) nor the
        // owned buffer change afterwards.
        unsafe { rkyv::access_unchecked::<ArchivedPackageIndex>(bytes) }
    }

    /// The number of files in the package.
    pub(crate) fn file_count(&self) -> usize {
        self.archived().files.len()
    }

    pub(crate) fn files(&self) -> impl Iterator<Item = FileRef<'_>> {
        self.archived().files.iter().map(|file| FileRef { file })
    }

    pub(crate) fn entry_points(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.archived().entry_points.iter().map(|ep| {
            (
                ep.command.as_str(),
                ep.module.as_str(),
                ep.function.as_str(),
            )
        })
    }
}

/// Which of the target properties the scans depend on hold for `platform`.
fn target_family(platform: Subdir) -> u8 {
    u8::from(platform.is_windows()) | (u8::from(platform.is_unix()) << 1)
}

/// Whether `file` (header and archive, as stored at [`INDEX_PATH`]) is an
/// intact index of the current version for a target like `platform` whose
/// file list still has the given stamp.
///
/// The checksum catches corruption that still forms a structurally valid
/// archive (a flipped bit in a path or an offset), which `rkyv` validation
/// alone would accept.
fn validate(file: &[u8], platform: Subdir, paths_json: &Stamp) -> bool {
    let Some((header, bytes)) = file.split_at_checked(HEADER_LEN) else {
        return false;
    };
    if &header[..8] != MAGIC || header[8..] != xxhash_rust::xxh3::xxh3_64(bytes).to_le_bytes() {
        return false;
    }
    let Ok(index) = rkyv::access::<ArchivedPackageIndex, rancor::Error>(bytes) else {
        return false;
    };
    index.version.to_native() == VERSION
        && index.target_family == target_family(platform)
        && index.paths_json.len.to_native() == paths_json.len
        && index.paths_json.mtime_secs.to_native() == paths_json.mtime_secs
        && index.paths_json.mtime_nanos.to_native() == paths_json.mtime_nanos
}

/// The stamp of the package's file list: `info/paths.json`, or the
/// deprecated `info/files` for very old packages.
fn file_list_stamp(package_dir: &Path) -> Option<Stamp> {
    let metadata = fs::metadata(package_dir.join("info/paths.json"))
        .or_else(|_| fs::metadata(package_dir.join("info/files")))
        .ok()?;
    let mtime = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(Stamp {
        len: metadata.len(),
        mtime_secs: mtime.as_secs(),
        mtime_nanos: mtime.subsec_nanos(),
    })
}

/// Load the index of the package extracted at `package_dir`, building (and
/// trying to store) it when there is no usable one.
///
/// Entry points are recorded whenever the package has a noarch Python
/// `link.json`, so the stored index doesn't depend on how the caller
/// classified the package; callers decide whether to use them.
pub(crate) fn load_or_build(package_dir: &Path, platform: Subdir) -> anyhow::Result<IndexBytes> {
    let stamp = file_list_stamp(package_dir);
    let index_path = package_dir.join(INDEX_PATH);

    if let Some(stamp) = &stamp
        && let Ok(file) = File::open(&index_path)
        // SAFETY: the index is only ever replaced by rename, never modified in
        // place, so the mapping stays valid.
        && let Ok(mmap) = unsafe { Mmap::map(&file) }
        && validate(&mmap, platform, stamp)
    {
        return Ok(IndexBytes::Mapped(mmap));
    }

    let paths_json = PathsJson::from_package_directory_with_deprecated_fallback(package_dir)?;
    let entry_points = LinkJson::from_package_directory(package_dir)
        .ok()
        .and_then(|link| match link.noarch {
            NoArchLinks::Python(ep) => Some(ep.entry_points),
            NoArchLinks::Generic => None,
        })
        .unwrap_or_default();
    let index = build(
        package_dir,
        &paths_json,
        entry_points
            .into_iter()
            .map(|ep| IndexEntryPoint {
                command: ep.command,
                module: ep.module,
                function: ep.function,
            })
            .collect(),
        platform,
        stamp.clone().unwrap_or(Stamp {
            len: 0,
            mtime_secs: 0,
            mtime_nanos: 0,
        }),
    );
    let bytes = rkyv::to_bytes::<rancor::Error>(&index)?;
    // Validate what we're about to hand out like a loaded index, so
    // `IndexBytes::archived` can skip validation.
    anyhow::ensure!(
        rkyv::access::<ArchivedPackageIndex, rancor::Error>(&bytes).is_ok(),
        "freshly built package index does not validate"
    );

    if stamp.is_some()
        && let Err(e) = store(&index_path, &bytes)
    {
        tracing::debug!("not caching package index {}: {e}", index_path.display());
    }
    Ok(IndexBytes::Owned(bytes))
}

/// Atomically write the archive `bytes`, behind a checksummed header, to
/// `path`.
fn store(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let mut header = [0u8; HEADER_LEN];
    header[..8].copy_from_slice(MAGIC);
    header[8..].copy_from_slice(&xxhash_rust::xxh3::xxh3_64(bytes).to_le_bytes());
    let result = File::create(&tmp)
        .and_then(|mut file| {
            file.write_all(&header)?;
            file.write_all(bytes)
        })
        .and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

fn build(
    package_dir: &Path,
    paths_json: &PathsJson,
    entry_points: Vec<IndexEntryPoint>,
    platform: Subdir,
    paths_json_stamp: Stamp,
) -> PackageIndex {
    // Files are independent and the expensive ones (scanning large binaries)
    // dominate, so index them in parallel.
    let files = paths_json
        .paths
        .par_iter()
        .map(|entry| index_file(package_dir, entry, platform))
        .collect();
    PackageIndex {
        version: VERSION,
        target_family: target_family(platform),
        paths_json: paths_json_stamp,
        entry_points,
        files,
    }
}

fn index_file(package_dir: &Path, entry: &PathsEntry, platform: Subdir) -> IndexFile {
    let path = package_dir.join(&entry.relative_path);
    let stat = fs::symlink_metadata(&path).ok().map(|metadata| {
        let kind = if metadata.is_dir() {
            FileKind::Directory
        } else if metadata.is_symlink() {
            FileKind::Symlink
        } else {
            FileKind::RegularFile
        };
        let attr = crate::vfs_ops::FileAttr::from_metadata(&metadata, 0);
        let split = |t: SystemTime| {
            let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
            (d.as_secs(), d.subsec_nanos())
        };
        let (mtime_secs, mtime_nanos) = split(attr.mtime);
        let (ctime_secs, ctime_nanos) = split(attr.ctime);
        let is_pyc = entry
            .relative_path
            .extension()
            .is_some_and(|ext| ext == "pyc");
        IndexStat {
            len: attr.size,
            blocks: attr.blocks,
            mtime_secs,
            mtime_nanos,
            ctime_secs,
            ctime_nanos,
            perm: attr.perm,
            uid: attr.uid,
            gid: attr.gid,
            kind: kind_to_u8(kind),
            pyc_header: (is_pyc && kind == FileKind::RegularFile)
                .then(|| read_pyc_header(&path))
                .flatten(),
        }
    });
    let placeholder = entry
        .prefix_placeholder
        .as_ref()
        .map(|placeholder| IndexPlaceholder {
            placeholder: placeholder.placeholder.clone(),
            text: placeholder.file_mode == FileMode::Text,
            scan: locate_placeholder(&path, placeholder, platform).map(|scan| match scan {
                ScanResult::Text {
                    region_end,
                    occurrences,
                } => IndexScan::Text {
                    region_end: region_end as u64,
                    occurrences: occurrences
                        .into_iter()
                        .map(|(offset, encoding)| (offset as u64, encoding_to_u8(encoding)))
                        .collect(),
                },
                ScanResult::Binary(cstrings) => IndexScan::Binary {
                    cstrings: cstrings
                        .into_iter()
                        .map(|c| {
                            (
                                encoding_to_u8(c.encoding),
                                c.offsets.into_iter().map(|o| o as u64).collect(),
                                c.nul_pos as u64,
                            )
                        })
                        .collect(),
                },
            }),
        });
    IndexFile {
        // Join the components with `/` rather than rewriting separators: on
        // Unix a backslash is an ordinary file-name character.
        path: entry
            .relative_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        path_type: match entry.path_type {
            PathType::HardLink => 0,
            PathType::SoftLink => 1,
            PathType::Directory => 2,
        },
        stat,
        placeholder,
    }
}

/// `(flags, source mtime, source size)` from a `.pyc` header.
fn read_pyc_header(path: &Path) -> Option<(u32, u32, u32)> {
    let mut header = [0u8; 16];
    File::open(path).ok()?.read_exact(&mut header).ok()?;
    let word = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
    Some((word(4), word(8), word(12)))
}

/// Where `placeholder` occurs in the file at `path`, preferring the offsets
/// recorded in `paths.json` and scanning the file otherwise. Mirrors the
/// installer: binaries are never patched on Windows.
pub(crate) fn locate_placeholder(
    path: &Path,
    placeholder: &rattler_conda_types::package::PrefixPlaceholder,
    platform: Subdir,
) -> Option<ScanResult> {
    let recorded = match &placeholder.experimental_offsets {
        Some(Ok(offsets)) if offsets.file_mode() == placeholder.file_mode => Some(offsets),
        Some(Ok(offsets)) => {
            tracing::warn!(
                "{}: offsets recorded for file mode {:?} but the file is {:?}; \
                 falling back to scanning",
                path.display(),
                offsets.file_mode(),
                placeholder.file_mode
            );
            None
        }
        Some(Err(e)) => {
            tracing::warn!(
                "{}: unusable offset metadata ({e}); falling back to scanning",
                path.display()
            );
            None
        }
        None => None,
    };

    match placeholder.file_mode {
        FileMode::Binary if platform.is_windows() => Some(ScanResult::Binary(Vec::new())),
        FileMode::Binary => match recorded {
            Some(offsets) => Some(ScanResult::Binary(
                crate::prefix_replacement::cstrings_from_recorded(offsets.groups()),
            )),
            None => scan(path, |source| {
                ScanResult::Binary(crate::prefix_replacement::plan_binary_replacement(
                    source,
                    &placeholder.placeholder,
                ))
            }),
        },
        FileMode::Text => {
            if let Some(offsets) = recorded
                && let Some(result) = text_from_recorded(path, offsets)
            {
                return Some(result);
            }
            scan(path, |source| {
                let (region_end, occurrences) = crate::prefix_replacement::scan_text(
                    source,
                    &placeholder.placeholder,
                    &platform,
                );
                ScanResult::Text {
                    region_end,
                    occurrences,
                }
            })
        }
    }
}

/// The recorded offsets of a text file, provided its recorded shebang region
/// really is one (the shebang transform requires the region to start with
/// `#!`).
fn text_from_recorded(path: &Path, offsets: &PrefixOffsets) -> Option<ScanResult> {
    let region_end = offsets.shebang_length().unwrap_or(0);
    if region_end > 0 {
        let mut region = Vec::with_capacity(2);
        File::open(path)
            .ok()?
            .take(2)
            .read_to_end(&mut region)
            .ok()?;
        if region != b"#!" {
            tracing::warn!(
                "{}: recorded shebang_length does not match the file contents; \
                 falling back to scanning",
                path.display()
            );
            return None;
        }
    }
    let mut occurrences: Vec<(usize, OffsetEncoding)> = offsets
        .groups()
        .iter()
        .filter_map(|group| match group.ranges() {
            OffsetRanges::Text(offsets) => Some((group.encoding(), offsets)),
            OffsetRanges::Binary(_) => None,
        })
        .flat_map(|(encoding, offsets)| offsets.iter().map(move |&offset| (offset, encoding)))
        .collect();
    occurrences.sort_by_key(|&(offset, _)| offset);
    Some(ScanResult::Text {
        region_end,
        occurrences,
    })
}

/// Run `f` over the contents of `path`, memory-mapped rather than read so a
/// large binary isn't copied to the heap.
fn scan(path: &Path, f: impl FnOnce(&[u8]) -> ScanResult) -> Option<ScanResult> {
    let warn = |e: &dyn std::fmt::Display| {
        tracing::warn!(
            "failed to read {} for offset computation: {}",
            path.display(),
            e
        );
    };
    let file = File::open(path).map_err(|e| warn(&e)).ok()?;
    // SAFETY: package cache files are immutable while in use.
    let source = unsafe { Mmap::map(&file) }.map_err(|e| warn(&e)).ok()?;
    Some(f(&source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::package::{PathsEntry, PrefixPlaceholder};
    use std::path::PathBuf;

    fn entry(path: &str, placeholder: Option<FileMode>) -> PathsEntry {
        PathsEntry {
            relative_path: PathBuf::from(path),
            path_type: PathType::HardLink,
            prefix_placeholder: placeholder.map(|file_mode| PrefixPlaceholder {
                file_mode,
                placeholder: "/old/prefix".to_string(),
                experimental_offsets: None,
            }),
            no_link: false,
            sha256: None,
            size_in_bytes: None,
        }
    }

    /// A package directory with a text and a binary placeholder file and a
    /// `.pyc`.
    fn package() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("info")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("lib/__pycache__")).unwrap();
        fs::write(
            root.join("bin/tool"),
            b"#!/old/prefix/bin/python\n# /old/prefix\n",
        )
        .unwrap();
        fs::write(root.join("lib/libx.so"), b"\x7fELF\0/old/prefix/lib\0tail").unwrap();
        let mut pyc = vec![0x42, 0x42, 0x0d, 0x0a];
        pyc.extend_from_slice(&0u32.to_le_bytes());
        pyc.extend_from_slice(&7u32.to_le_bytes());
        pyc.extend_from_slice(&9u32.to_le_bytes());
        fs::write(root.join("lib/__pycache__/m.cpython-312.pyc"), pyc).unwrap();
        let paths = PathsJson {
            paths: vec![
                entry("bin/tool", Some(FileMode::Text)),
                entry("lib/libx.so", Some(FileMode::Binary)),
                entry("lib/__pycache__/m.cpython-312.pyc", None),
            ],
            paths_version: 1,
        };
        fs::write(
            root.join("info/paths.json"),
            serde_json::to_vec(&paths).unwrap(),
        )
        .unwrap();
        dir
    }

    #[test]
    fn builds_stores_and_reloads() {
        let dir = package();
        let built = load_or_build(dir.path(), Subdir::Linux64).unwrap();
        assert!(matches!(built, IndexBytes::Owned(_)));
        assert!(dir.path().join(INDEX_PATH).exists());

        assert_eq!(built.files().count(), 3);
        let file = |path: &str| built.files().find(|f| f.path() == path).unwrap();
        let (placeholder, mode, scan) = file("bin/tool").placeholder().unwrap();
        assert_eq!((placeholder, mode), ("/old/prefix", FileMode::Text));
        assert_eq!(
            scan,
            Some(ScanResult::Text {
                region_end: 25,
                occurrences: vec![(27, OffsetEncoding::Utf8)],
            })
        );
        let (_, mode, scan) = file("lib/libx.so").placeholder().unwrap();
        assert_eq!(mode, FileMode::Binary);
        let Some(ScanResult::Binary(cstrings)) = scan else {
            panic!("expected a binary scan");
        };
        assert_eq!(cstrings.len(), 1);
        assert_eq!(cstrings[0].offsets, vec![5]);
        assert_eq!(
            file("lib/__pycache__/m.cpython-312.pyc")
                .stat()
                .unwrap()
                .pyc_header,
            Some((0, 7, 9))
        );
        assert_eq!(file("bin/tool").stat().unwrap().len, 39);

        // A second load maps the stored index and sees the same data.
        let loaded = load_or_build(dir.path(), Subdir::Linux64).unwrap();
        assert!(matches!(loaded, IndexBytes::Mapped(_)));
        assert_eq!(
            loaded.files().map(|f| f.placeholder()).collect::<Vec<_>>(),
            built.files().map(|f| f.placeholder()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rebuilds_when_stale_or_corrupt() {
        let dir = package();
        load_or_build(dir.path(), Subdir::Linux64).unwrap();

        // Another target family invalidates the scans.
        let other = load_or_build(dir.path(), Subdir::Win64).unwrap();
        assert!(matches!(other, IndexBytes::Owned(_)));
        let (_, _, scan) = other
            .files()
            .find(|f| f.path() == "lib/libx.so")
            .unwrap()
            .placeholder()
            .unwrap();
        assert_eq!(scan, Some(ScanResult::Binary(Vec::new())));

        // A corrupt index is rebuilt.
        fs::write(dir.path().join(INDEX_PATH), b"garbage").unwrap();
        let rebuilt = load_or_build(dir.path(), Subdir::Linux64).unwrap();
        assert!(matches!(rebuilt, IndexBytes::Owned(_)));
        assert_eq!(rebuilt.files().count(), 3);

        // A changed file list is rebuilt too.
        let paths_json = dir.path().join("info/paths.json");
        let mut contents = fs::read(&paths_json).unwrap();
        contents.push(b' ');
        fs::write(&paths_json, contents).unwrap();
        let rebuilt = load_or_build(dir.path(), Subdir::Linux64).unwrap();
        assert!(matches!(rebuilt, IndexBytes::Owned(_)));
    }

    #[test]
    fn rebuilds_when_a_bit_flips() {
        let dir = package();
        load_or_build(dir.path(), Subdir::Linux64).unwrap();
        let index_path = dir.path().join(INDEX_PATH);
        let mut bytes = fs::read(&index_path).unwrap();
        // Corrupt a path string inside the archive: still a structurally
        // valid archive, but the checksum no longer matches.
        let pos = bytes
            .windows(8)
            .position(|w| w == b"bin/tool")
            .expect("path stored in the archive");
        bytes[pos] ^= 0x01;
        fs::write(&index_path, &bytes).unwrap();
        let rebuilt = load_or_build(dir.path(), Subdir::Linux64).unwrap();
        assert!(matches!(rebuilt, IndexBytes::Owned(_)));
        assert!(rebuilt.files().any(|f| f.path() == "bin/tool"));
    }

    #[cfg(unix)]
    #[test]
    fn keeps_backslashes_in_unix_file_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("info")).unwrap();
        fs::create_dir_all(root.join("lib/systemd")).unwrap();
        fs::write(root.join("lib/systemd/a\\x2db.mount"), b"unit").unwrap();
        let paths = PathsJson {
            paths: vec![entry("lib/systemd/a\\x2db.mount", None)],
            paths_version: 1,
        };
        fs::write(
            root.join("info/paths.json"),
            serde_json::to_vec(&paths).unwrap(),
        )
        .unwrap();
        let index = load_or_build(root, Subdir::Linux64).unwrap();
        let file = index.files().next().unwrap();
        assert_eq!(file.path(), "lib/systemd/a\\x2db.mount");
        assert_eq!(file.stat().unwrap().len, 4);
    }

    #[test]
    fn records_entry_points_of_noarch_python_packages() {
        let dir = package();
        fs::write(
            dir.path().join("info/link.json"),
            br#"{"noarch": {"type": "python", "entry_points": ["tool = pkg.cli:main"]}, "package_metadata_version": 1}"#,
        )
        .unwrap();
        for _ in 0..2 {
            let index = load_or_build(dir.path(), Subdir::Linux64).unwrap();
            assert_eq!(
                index.entry_points().collect::<Vec<_>>(),
                vec![("tool", "pkg.cli", "main")]
            );
        }
    }
}
