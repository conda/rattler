//! Cache for parsed package metadata.
//!
//! The cache serves two purposes:
//!
//! 1. **Retries within a run.** When indexing is retried because another
//!    process modified the repodata concurrently, previously parsed packages
//!    are reused instead of being downloaded and parsed again.
//! 2. **Resumability across runs.** With a [`PackageRecordCache::with_store`]
//!    backing file, every parsed package is appended to disk as soon as it is
//!    available. A run that is interrupted (crash, `SIGINT`, out of memory)
//!    loses only the packages that were in flight, and the next run picks up
//!    where it left off.
//!
//! Entries are validated against the current file metadata (`ETag`, then
//! `last_modified`, then size) before they are used, so a replaced package is
//! always re-parsed. Packages that could not be parsed are recorded as broken so
//! they are not downloaded again until the file changes.
//!
//! The cache does not store the derived [`rattler_conda_types::PackageRecord`].
//! It stores the raw `info/index.json`, `info/run_exports.json` and the archive
//! digests, and re-derives the record on every hit. A cached package is
//! therefore indistinguishable from a freshly parsed one, and changes to how
//! records are derived apply to cached packages as well.
//!
//! Each subdir gets its own cache instance. It works with any `OpenDAL` backend,
//! using conditional reads when supported (S3, HTTP) or simple reads as fallback
//! (filesystem).

use std::{
    collections::HashSet,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex as StdMutex},
    time::SystemTime,
};

use fs_err as fs;
use opendal::{Operator, raw::Timestamp};
use rattler_networking::retry_policies::default_retry_policy;
use retry_policies::{RetryDecision, RetryPolicy};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{ParsedPackage, RepodataFileMetadata};

/// Version of the on-disk cache format. Lines with a different version are
/// ignored when loading.
const CACHE_FORMAT_VERSION: u32 = 1;

/// File metadata used to validate a cache entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CachedFileMetadata {
    /// The `ETag` when this entry was computed (if available)
    pub etag: Option<String>,
    /// The last modified time when this entry was computed (if available)
    pub last_modified: Option<Timestamp>,
    /// The size of the file when this entry was computed (if available)
    pub size: Option<u64>,
}

impl CachedFileMetadata {
    fn from_opendal(metadata: &opendal::Metadata) -> Self {
        Self {
            etag: metadata.etag().map(str::to_owned),
            last_modified: metadata.last_modified(),
            size: Some(metadata.content_length()),
        }
    }

    /// Returns `true` if `self` (the cached metadata) still describes a file
    /// with `current` metadata.
    ///
    /// Prefers the `ETag`, falls back to `last_modified`, then to the size. If
    /// none of the fields are available on both sides the entry cannot be
    /// validated and is treated as stale.
    fn matches(&self, current: &Self) -> bool {
        if let (Some(cached), Some(current)) = (&self.etag, &current.etag) {
            return cached == current;
        }
        if let (Some(cached), Some(current)) = (self.last_modified, current.last_modified) {
            return cached == current;
        }
        if let (Some(cached), Some(current)) = (self.size, current.size) {
            return cached == current;
        }
        false
    }
}

/// What the cache knows about a package file.
#[derive(Debug, Clone)]
enum CachedOutcome {
    /// The package was parsed successfully.
    Parsed(Arc<ParsedPackage>),
    /// The package could not be parsed. Holds the error message.
    Broken(String),
}

#[derive(Debug, Clone)]
struct CachedPackage {
    metadata: CachedFileMetadata,
    outcome: CachedOutcome,
}

/// Result of a cache lookup operation from [`PackageRecordCache::get_or_stat`].
#[derive(Debug)]
pub(crate) enum CacheResult {
    /// Cache hit - the cached package is still valid (metadata matches).
    Hit(Arc<ParsedPackage>),

    /// Cache hit - the file is known to be unparsable and has not changed.
    Broken(String),

    /// Cache miss - need to read and parse the file.
    /// Contains current file metadata for conditional reading.
    Miss(CachedFileMetadata),
}

/// One line of the on-disk cache file.
#[derive(Debug, Serialize, Deserialize)]
struct CacheLine {
    v: u32,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package: Option<ParsedPackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl CacheLine {
    fn new(path: &str, cached: &CachedPackage) -> Self {
        let (package, error) = match &cached.outcome {
            CachedOutcome::Parsed(package) => (Some(ParsedPackage::clone(package)), None),
            CachedOutcome::Broken(error) => (None, Some(error.clone())),
        };
        Self {
            v: CACHE_FORMAT_VERSION,
            path: path.to_owned(),
            etag: cached.metadata.etag.clone(),
            last_modified: cached.metadata.last_modified.map(|ts| ts.to_string()),
            size: cached.metadata.size,
            package,
            error,
        }
    }

    fn into_entry(self) -> Option<(String, CachedPackage)> {
        if self.v != CACHE_FORMAT_VERSION {
            return None;
        }
        let outcome = match (self.package, self.error) {
            (Some(package), _) => CachedOutcome::Parsed(Arc::new(package)),
            (None, Some(error)) => CachedOutcome::Broken(error),
            (None, None) => return None,
        };
        let last_modified = self
            .last_modified
            .as_deref()
            .and_then(|ts| Timestamp::from_str(ts).ok());
        Some((
            self.path,
            CachedPackage {
                metadata: CachedFileMetadata {
                    etag: self.etag,
                    last_modified,
                    size: self.size,
                },
                outcome,
            },
        ))
    }
}

/// The on-disk backing store of a cache: an append-only file of JSON lines.
#[derive(Debug)]
struct DiskStore {
    path: PathBuf,
    /// Writer for appending new entries. `None` after the store has been
    /// compacted or if opening the file for appending failed.
    writer: StdMutex<Option<BufWriter<fs::File>>>,
    /// Whether entries were appended since the last compaction.
    dirty: StdMutex<bool>,
}

impl DiskStore {
    fn append(&self, line: &CacheLine) -> std::io::Result<()> {
        let mut guard = self.writer.lock().expect("cache writer lock poisoned");
        let Some(writer) = guard.as_mut() else {
            return Ok(());
        };
        serde_json::to_writer(&mut *writer, line)?;
        writer.write_all(b"\n")?;
        // Flush on every append: entries are small compared to the package
        // downloads they represent, and an interrupted run must not lose them.
        writer.flush()?;
        *self.dirty.lock().expect("cache dirty lock poisoned") = true;
        Ok(())
    }
}

/// Cache for parsed packages keyed by file path.
///
/// Thread-safe with `Arc<RwLock<>>` - cheap to clone, all clones share the same
/// storage.
#[derive(Debug, Clone, Default)]
pub struct PackageRecordCache {
    inner: Arc<RwLock<ahash::HashMap<String, CachedPackage>>>,
    store: Option<Arc<DiskStore>>,
}

impl PackageRecordCache {
    /// Create a new in-memory cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cache backed by the JSON lines file at `path`.
    ///
    /// Existing entries are loaded from the file. Malformed lines and lines
    /// written by a different format version are skipped with a warning. The
    /// parent directory is created if it does not exist.
    pub fn with_store(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut entries: ahash::HashMap<String, CachedPackage> = ahash::HashMap::default();
        match fs::File::open(&path) {
            Ok(file) => {
                let mut skipped = 0usize;
                for (line_number, line) in BufReader::new(file).lines().enumerate() {
                    let line = line?;
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<CacheLine>(&line) {
                        Ok(parsed) => match parsed.into_entry() {
                            Some((path, cached)) => {
                                // Later lines override earlier ones.
                                entries.insert(path, cached);
                            }
                            None => skipped += 1,
                        },
                        Err(err) => {
                            skipped += 1;
                            tracing::debug!(
                                "Skipping malformed line {} in cache file {}: {err}",
                                line_number + 1,
                                path.display()
                            );
                        }
                    }
                }
                if skipped > 0 {
                    tracing::warn!(
                        "Skipped {skipped} unreadable entries in cache file {}",
                        path.display()
                    );
                }
                tracing::info!(
                    "Loaded {} cached package entries from {}",
                    entries.len(),
                    path.display()
                );
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }

        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        Ok(Self {
            inner: Arc::new(RwLock::new(entries)),
            store: Some(Arc::new(DiskStore {
                path,
                writer: StdMutex::new(Some(BufWriter::new(file))),
                dirty: StdMutex::new(false),
            })),
        })
    }

    /// The path of the backing file, if this cache is persisted.
    pub fn store_path(&self) -> Option<&Path> {
        self.store.as_ref().map(|store| store.path.as_path())
    }

    /// The number of entries in the cache.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Whether the cache holds no entries.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Get a cached package if valid, or return current file metadata.
    ///
    /// Performs a `stat()` to get current metadata, then validates any cached
    /// entry. Returns [`CacheResult::Hit`] or [`CacheResult::Broken`] if the
    /// metadata matches, or [`CacheResult::Miss`] with current metadata
    /// otherwise.
    pub(crate) async fn get_or_stat(
        &self,
        op: &Operator,
        path: &str,
    ) -> opendal::Result<CacheResult> {
        let metadata = match op.stat(path).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    "Failed to stat file during cache lookup for {}: {}",
                    path,
                    e
                );
                return Err(e);
            }
        };
        let current = CachedFileMetadata::from_opendal(&metadata);

        let cached = {
            let guard = self.inner.read().await;
            guard.get(path).cloned()
        };

        match cached {
            Some(cached) if cached.metadata.matches(&current) => {
                tracing::debug!("Cache hit for {path}");
                Ok(match cached.outcome {
                    CachedOutcome::Parsed(package) => CacheResult::Hit(package),
                    CachedOutcome::Broken(error) => CacheResult::Broken(error),
                })
            }
            Some(_) => {
                tracing::debug!("Cache entry for {path} is stale, treating as miss");
                Ok(CacheResult::Miss(current))
            }
            None => {
                tracing::debug!("Cache miss for {path} (not in cache)");
                Ok(CacheResult::Miss(current))
            }
        }
    }

    async fn store(&self, path: &str, cached: CachedPackage) {
        if let Some(store) = &self.store {
            let line = CacheLine::new(path, &cached);
            // Appending is a small synchronous write; do it before taking the
            // async lock so a slow disk does not hold up other lookups.
            if let Err(err) = store.append(&line) {
                tracing::warn!(
                    "Failed to append {path} to cache file {}: {err}",
                    store.path.display()
                );
            }
        }
        let mut guard = self.inner.write().await;
        guard.insert(path.to_string(), cached);
    }

    /// Insert a parsed package into the cache with its file metadata.
    ///
    /// Use metadata from the actual read operation (especially important with
    /// [`read_package_with_retry`] which updates metadata during retries).
    pub(crate) async fn insert(
        &self,
        path: &str,
        package: Arc<ParsedPackage>,
        metadata: CachedFileMetadata,
    ) {
        self.store(
            path,
            CachedPackage {
                metadata,
                outcome: CachedOutcome::Parsed(package),
            },
        )
        .await;
    }

    /// Record that the package at `path` could not be parsed.
    ///
    /// The package is not downloaded again until its metadata changes.
    pub(crate) async fn insert_broken(
        &self,
        path: &str,
        error: String,
        metadata: CachedFileMetadata,
    ) {
        self.store(
            path,
            CachedPackage {
                metadata,
                outcome: CachedOutcome::Broken(error),
            },
        )
        .await;
    }

    /// Rewrite the backing file so it contains exactly one line per entry.
    ///
    /// Entries whose path is not in `keep` are dropped, which removes packages
    /// that no longer exist in the channel. Does nothing for an in-memory cache
    /// or if nothing was appended since the last compaction and no entries are
    /// dropped. The file is replaced atomically.
    pub async fn compact(&self, keep: &HashSet<String>) -> std::io::Result<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };

        let mut entries = self.inner.write().await;
        let before = entries.len();
        entries.retain(|path, _| keep.contains(path));
        let dirty = *store.dirty.lock().expect("cache dirty lock poisoned");
        if !dirty && entries.len() == before {
            return Ok(());
        }

        let lines = entries
            .iter()
            .map(|(path, cached)| CacheLine::new(path, cached))
            .collect::<Vec<_>>();
        drop(entries);

        let tmp_path = store.path.with_extension("jsonl.tmp");
        {
            let mut writer = BufWriter::new(fs::File::create(&tmp_path)?);
            for line in &lines {
                serde_json::to_writer(&mut writer, line)?;
                writer.write_all(b"\n")?;
            }
            writer.flush()?;
            writer.get_ref().sync_all()?;
        }

        // Swap the append writer for one on the new file before renaming so no
        // append lands in the old file after the rename.
        let mut writer = store.writer.lock().expect("cache writer lock poisoned");
        fs::rename(&tmp_path, &store.path)?;
        let file = fs::OpenOptions::new().append(true).open(&store.path)?;
        *writer = Some(BufWriter::new(file));
        *store.dirty.lock().expect("cache dirty lock poisoned") = false;
        tracing::debug!(
            "Compacted cache file {} to {} entries",
            store.path.display(),
            lines.len()
        );
        Ok(())
    }
}

/// Read a package file with retry logic for handling concurrent modifications.
///
/// Uses conditional requests (`if-match`/`if-unmodified-since`) to ensure reading the
/// same version that was stated. Retries with exponential backoff if the file changes
/// between `stat()` and `read()`.
///
/// Only retries on [`ErrorKind::ConditionNotMatch`]. Falls back to simple `read()` for
/// backends without conditional read support (returns [`ErrorKind::Unsupported`]).
///
/// Returns `(buffer, final_metadata)` where `final_metadata` reflects the version actually
/// read (may differ from `initial_metadata` if retries occurred).
///
/// [`ErrorKind::ConditionNotMatch`]: opendal::ErrorKind::ConditionNotMatch
/// [`ErrorKind::Unsupported`]: opendal::ErrorKind::Unsupported
pub async fn read_package_with_retry(
    op: &Operator,
    path: &str,
    initial_metadata: RepodataFileMetadata,
) -> opendal::Result<(opendal::Buffer, RepodataFileMetadata)> {
    let retry_policy = default_retry_policy();
    let mut current_try = 0;
    let mut metadata = initial_metadata;

    loop {
        let request_start_time = SystemTime::now();

        // Try to read the file with conditional checks
        match crate::utils::read_with_metadata_check(op, path, &metadata).await {
            Ok(buffer) => return Ok((buffer, metadata)),
            Err(e) if e.kind() == opendal::ErrorKind::Unsupported => {
                // Backend doesn't support conditional reads (e.g., filesystem) -
                // fall back to simple read without retry logic
                tracing::debug!(
                    "Conditional reads not supported for {}, using simple read",
                    path
                );
                let buffer = op.read(path).await?;
                return Ok((buffer, metadata));
            }
            Err(e) if e.kind() == opendal::ErrorKind::ConditionNotMatch => {
                // File changed - check if we should retry
                match retry_policy.should_retry(request_start_time, current_try) {
                    RetryDecision::Retry { execute_after } => {
                        let duration = execute_after
                            .duration_since(SystemTime::now())
                            .unwrap_or_default();
                        tracing::debug!(
                            "File {} changed between stat and read (attempt {}), retrying in {:?}",
                            path,
                            current_try + 1,
                            duration
                        );
                        tokio::time::sleep(duration).await;
                        current_try += 1;

                        // Re-stat the file to get fresh metadata for next iteration
                        let fresh_metadata = op.stat(path).await?;
                        metadata = RepodataFileMetadata {
                            etag: fresh_metadata.etag().map(str::to_owned),
                            last_modified: fresh_metadata.last_modified(),
                            file_existed: true,
                            precondition_checks: crate::PreconditionChecks::Enabled,
                        };
                        // Loop continues to next iteration with fresh metadata
                    }
                    RetryDecision::DoNotRetry => {
                        tracing::warn!(
                            "Max retries exceeded for reading {} due to concurrent modifications",
                            path
                        );
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                // Not a retryable error - propagate immediately
                return Err(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_creation() {
        let cache = PackageRecordCache::new();
        assert!(cache.inner.try_read().is_ok());
        assert!(cache.store_path().is_none());
    }

    #[test]
    fn test_metadata_matching_prefers_etag() {
        let cached = CachedFileMetadata {
            etag: Some("a".into()),
            last_modified: None,
            size: Some(1),
        };
        let same_etag_other_size = CachedFileMetadata {
            etag: Some("a".into()),
            last_modified: None,
            size: Some(2),
        };
        assert!(cached.matches(&same_etag_other_size));

        let other_etag = CachedFileMetadata {
            etag: Some("b".into()),
            ..cached.clone()
        };
        assert!(!cached.matches(&other_etag));
    }

    #[test]
    fn test_metadata_matching_falls_back_to_size() {
        let cached = CachedFileMetadata {
            etag: None,
            last_modified: None,
            size: Some(1),
        };
        assert!(cached.matches(&cached.clone()));
        assert!(!cached.matches(&CachedFileMetadata {
            size: Some(2),
            ..cached.clone()
        }));
        assert!(!CachedFileMetadata::default().matches(&CachedFileMetadata::default()));
    }
}
