//! Persistent cache of prefix-placeholder scan results.
//!
//! Most packages don't record placeholder offsets in `paths.json` yet, so
//! [`crate::virtual_fs::VirtualFS`] locates the placeholder by reading every
//! prefix-bearing file in full when it is constructed. For a typical Python
//! environment that is tens of megabytes (the interpreter binary and
//! `libpython` alone are ~30 MB each), read again on every mount.
//!
//! The scan result only depends on the file contents, the placeholder and the
//! platform, not on the mount point, so it can be stored and reused: a cache
//! file maps each scanned package-cache file to the offsets found in it. An
//! entry is only used while the file's size and modification time still match,
//! so a re-extracted package is scanned again.
//!
//! The cache is an optimization only. A missing, unreadable or corrupt cache
//! file means files are scanned as if there were no cache, and failing to
//! write it is logged and otherwise ignored.

use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use rattler::install::link::CStringOccurrences;
use rattler_conda_types::{Subdir, package::OffsetEncoding};
use serde::{Deserialize, Serialize};

/// Bump when the format or the semantics of a stored scan change.
const VERSION: u32 = 1;

/// Where the placeholder occurs in one file, independent of the target prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScanResult {
    /// See [`crate::prefix_replacement::scan_text`].
    Text {
        region_end: usize,
        occurrences: Vec<(usize, OffsetEncoding)>,
    },
    /// See [`crate::prefix_replacement::plan_binary_replacement`].
    Binary(Vec<CStringOccurrences>),
}

/// The identity of a scanned file: if any of this changes, the stored scan
/// no longer applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileStamp {
    len: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
}

impl FileStamp {
    pub(crate) fn of(metadata: &fs::Metadata) -> Option<Self> {
        let mtime = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            len: metadata.len(),
            mtime_secs: mtime.as_secs(),
            mtime_nanos: mtime.subsec_nanos(),
        })
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }
}

/// A cached scan of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanEntry {
    pub(crate) stamp: FileStamp,
    pub(crate) placeholder: String,
    pub(crate) result: ScanResult,
}

/// The scan results of the files of one mount.
#[derive(Debug, Default)]
pub(crate) struct ScanCache {
    entries: HashMap<PathBuf, ScanEntry>,
}

impl ScanCache {
    /// Load the cache stored at `path` for `platform`. Anything unusable
    /// yields an empty cache.
    pub(crate) fn load(path: &Path, platform: Subdir) -> Self {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::debug!("ignoring unreadable scan cache {}: {e}", path.display());
                return Self::default();
            }
        };
        let file: CacheFile = match serde_json::from_slice(&bytes) {
            Ok(file) => file,
            Err(e) => {
                tracing::debug!("ignoring corrupt scan cache {}: {e}", path.display());
                return Self::default();
            }
        };
        if file.version != VERSION || file.platform != platform.as_str() {
            return Self::default();
        }
        let entries = file
            .entries
            .into_iter()
            .filter_map(|(path, entry)| Some((path, entry.into_entry()?)))
            .collect();
        Self { entries }
    }

    /// The stored scan of `path`, if it was made for `placeholder` and the
    /// file still has the given `stamp`.
    pub(crate) fn get(
        &self,
        path: &Path,
        placeholder: &str,
        stamp: &FileStamp,
    ) -> Option<&ScanResult> {
        let entry = self.entries.get(path)?;
        (entry.placeholder == placeholder && entry.stamp == *stamp).then_some(&entry.result)
    }

    /// Atomically replace the cache at `path` with `entries`.
    pub(crate) fn store(
        path: &Path,
        platform: Subdir,
        entries: impl IntoIterator<Item = (PathBuf, ScanEntry)>,
    ) -> std::io::Result<()> {
        let file = CacheFile {
            version: VERSION,
            platform: platform.as_str().to_string(),
            entries: entries
                .into_iter()
                .map(|(path, entry)| (path, StoredEntry::from_entry(&entry)))
                .collect(),
        };
        let bytes = serde_json::to_vec(&file).map_err(std::io::Error::other)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        // Write next to the destination and rename over it, so concurrent
        // mounts never observe a partially written cache.
        let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(format!(".{}.tmp", std::process::id()));
        let tmp = dir.join(tmp_name);
        let result = fs::File::create(&tmp)
            .and_then(|mut file| file.write_all(&bytes))
            .and_then(|()| fs::rename(&tmp, path));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

// --- On-disk format ---------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    platform: String,
    entries: HashMap<PathBuf, StoredEntry>,
}

#[derive(Serialize, Deserialize)]
struct StoredEntry {
    stamp: FileStamp,
    placeholder: String,
    #[serde(flatten)]
    result: StoredResult,
}

/// Encodings are stored by their CEP name (see [`OffsetEncoding::as_str`]).
#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum StoredResult {
    Text {
        region_end: usize,
        occurrences: Vec<(usize, String)>,
    },
    Binary {
        /// `(encoding, offsets, nul_pos)` per c-string.
        cstrings: Vec<(String, Vec<usize>, usize)>,
    },
}

impl StoredEntry {
    fn from_entry(entry: &ScanEntry) -> Self {
        let result = match &entry.result {
            ScanResult::Text {
                region_end,
                occurrences,
            } => StoredResult::Text {
                region_end: *region_end,
                occurrences: occurrences
                    .iter()
                    .map(|(offset, encoding)| (*offset, encoding.as_str().to_string()))
                    .collect(),
            },
            ScanResult::Binary(cstrings) => StoredResult::Binary {
                cstrings: cstrings
                    .iter()
                    .map(|c| {
                        (
                            c.encoding.as_str().to_string(),
                            c.offsets.clone(),
                            c.nul_pos,
                        )
                    })
                    .collect(),
            },
        };
        Self {
            stamp: entry.stamp.clone(),
            placeholder: entry.placeholder.clone(),
            result,
        }
    }

    fn into_entry(self) -> Option<ScanEntry> {
        let result = match self.result {
            StoredResult::Text {
                region_end,
                occurrences,
            } => ScanResult::Text {
                region_end,
                occurrences: occurrences
                    .into_iter()
                    .map(|(offset, encoding)| Some((offset, encoding.parse().ok()?)))
                    .collect::<Option<_>>()?,
            },
            StoredResult::Binary { cstrings } => ScanResult::Binary(
                cstrings
                    .into_iter()
                    .map(|(encoding, offsets, nul_pos)| {
                        Some(CStringOccurrences {
                            offsets,
                            nul_pos,
                            encoding: encoding.parse().ok()?,
                        })
                    })
                    .collect::<Option<_>>()?,
            ),
        };
        Some(ScanEntry {
            stamp: self.stamp,
            placeholder: self.placeholder,
            result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(len: u64) -> FileStamp {
        FileStamp {
            len,
            mtime_secs: 1_700_000_000,
            mtime_nanos: 5,
        }
    }

    #[test]
    fn roundtrip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("scan.json");
        let text_path = PathBuf::from("/pkgs/a/bin/script");
        let binary_path = PathBuf::from("/pkgs/a/lib/libfoo.so");
        let text = ScanEntry {
            stamp: stamp(10),
            placeholder: "/opt/placeholder".into(),
            result: ScanResult::Text {
                region_end: 7,
                occurrences: vec![(9, OffsetEncoding::Utf8), (20, OffsetEncoding::Utf16Le)],
            },
        };
        let binary = ScanEntry {
            stamp: stamp(99),
            placeholder: "/opt/placeholder".into(),
            result: ScanResult::Binary(vec![CStringOccurrences {
                offsets: vec![3, 40],
                nul_pos: 70,
                encoding: OffsetEncoding::Utf8,
            }]),
        };
        ScanCache::store(
            &cache_path,
            Subdir::Linux64,
            [
                (text_path.clone(), text.clone()),
                (binary_path.clone(), binary.clone()),
            ],
        )
        .unwrap();

        let cache = ScanCache::load(&cache_path, Subdir::Linux64);
        assert_eq!(
            cache.get(&text_path, "/opt/placeholder", &stamp(10)),
            Some(&text.result)
        );
        assert_eq!(
            cache.get(&binary_path, "/opt/placeholder", &stamp(99)),
            Some(&binary.result)
        );
        // A changed file, another placeholder or an unknown path miss.
        assert_eq!(cache.get(&text_path, "/opt/placeholder", &stamp(11)), None);
        assert_eq!(cache.get(&text_path, "/other", &stamp(10)), None);
        assert_eq!(
            cache.get(Path::new("/nope"), "/opt/placeholder", &stamp(10)),
            None
        );

        // A cache written for another platform is ignored.
        let other = ScanCache::load(&cache_path, Subdir::Win64);
        assert_eq!(other.get(&text_path, "/opt/placeholder", &stamp(10)), None);
    }

    #[test]
    fn missing_or_corrupt_cache_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.json");
        assert!(ScanCache::load(&path, Subdir::Linux64).entries.is_empty());
        fs::write(&path, b"not json").unwrap();
        assert!(ScanCache::load(&path, Subdir::Linux64).entries.is_empty());
    }
}
