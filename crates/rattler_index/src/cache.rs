//! Cache for parsed package metadata.
//!
//! The cache serves two purposes:
//!
//! 1. **Retries within a run.** When indexing is retried because another
//!    process modified the repodata concurrently, previously parsed packages
//!    are reused instead of being downloaded and parsed again.
//! 2. **Resumability across runs and machines.** With a channel store
//!    ([`PackageRecordCache::with_channel_store`]) every parsed package is
//!    written to the channel itself, as one small object per package under
//!    `<subdir>/.cache/`. A run that is interrupted (crash, `SIGINT`, out of
//!    memory) loses only the packages that were in flight, and the next run,
//!    on any machine, picks up where it left off.
//!
//! Several indexers may work on the same channel at the same time. The store
//! needs no coordination for that: the name of a cache object encodes the
//! metadata (`ETag`, else modification time and size) of the package it was
//! derived from, so a single listing tells every indexer which entries are
//! current, and two indexers writing the same entry produce identical objects,
//! written with `if-not-exists` so the loser of the race is a no-op.
//!
//! Packages that could not be parsed are recorded as broken so they are not
//! downloaded again until the file changes.
//!
//! The cache does not store the derived [`rattler_conda_types::PackageRecord`].
//! It stores the `info/index.json`, `info/run_exports.json` and the archive
//! digests, and re-derives the record on every hit. A cached package is
//! therefore indistinguishable from a freshly parsed one, and changes to how
//! records are derived apply to cached packages as well.
//!
//! Each subdir gets its own cache instance. It works with any `OpenDAL` backend,
//! using conditional reads when supported (S3, HTTP) or simple reads as fallback
//! (filesystem).

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::SystemTime,
};

use opendal::{Operator, raw::Timestamp};
use rattler_conda_types::Subdir;
use rattler_networking::retry_policies::default_retry_policy;
use retry_policies::{RetryDecision, RetryPolicy};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{ParsedPackage, RepodataFileMetadata};

/// Name of the directory inside a subdir that holds the cache objects.
pub const CACHE_DIR: &str = ".cache";

/// Version of the cache object format. Objects with a different version are
/// treated as misses.
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
            // opendal reports `0` when the backend did not provide a length. A
            // package archive is never empty, so treat `0` as unknown.
            size: Some(metadata.content_length()).filter(|size| *size > 0),
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

    /// A short identifier of this version of the file, used in the name of
    /// its cache object. Two machines that see the same file compute the same
    /// tag. `None` if the backend provided nothing to identify the version.
    ///
    /// Tags never contain a `.`, so the package filename can be split off the
    /// object name unambiguously.
    fn tag(&self) -> Option<String> {
        if let Some(etag) = &self.etag {
            let etag = etag.trim_matches('"');
            let safe = etag
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            return Some(if safe {
                format!("etag-{etag}")
            } else {
                format!("etagx-{}", hex::encode(etag))
            });
        }
        match (self.last_modified, self.size) {
            (Some(modified), size) => {
                let nanos = modified.into_inner().as_nanosecond();
                Some(match size {
                    Some(size) => format!("mtime{nanos}-size{size}"),
                    None => format!("mtime{nanos}"),
                })
            }
            (None, Some(size)) => Some(format!("size{size}")),
            (None, None) => None,
        }
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

/// The body of one cache object.
#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package: Option<ParsedPackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl CacheEntry {
    fn new(cached: &CachedPackage) -> Self {
        let (package, error) = match &cached.outcome {
            CachedOutcome::Parsed(package) => (Some(ParsedPackage::clone(package)), None),
            CachedOutcome::Broken(error) => (None, Some(error.clone())),
        };
        Self {
            version: CACHE_FORMAT_VERSION,
            etag: cached.metadata.etag.clone(),
            last_modified: cached.metadata.last_modified.map(Timestamp::into_inner),
            size: cached.metadata.size,
            package,
            error,
        }
    }

    fn into_cached(self) -> Option<CachedPackage> {
        if self.version != CACHE_FORMAT_VERSION {
            return None;
        }
        let outcome = match (self.package, self.error) {
            (Some(package), _) => CachedOutcome::Parsed(Arc::new(package)),
            (None, Some(error)) => CachedOutcome::Broken(error),
            (None, None) => return None,
        };
        Some(CachedPackage {
            metadata: CachedFileMetadata {
                etag: self.etag,
                last_modified: self.last_modified.map(Timestamp::from),
                size: self.size,
            },
            outcome,
        })
    }
}

/// The cache objects of one subdir, stored in the channel under
/// `<subdir>/.cache/<package filename>.<tag>.json`.
#[derive(Debug)]
struct ChannelStore {
    op: Operator,
    prefix: String,
    /// Object tags known to exist, per package filename. Filled from one
    /// listing when the store is opened and kept up to date with our writes.
    listed: RwLock<HashMap<String, HashSet<String>>>,
}

impl ChannelStore {
    async fn open(op: Operator, subdir: Subdir) -> opendal::Result<Self> {
        let prefix = format!("{}/{CACHE_DIR}/", subdir.as_str());
        let mut listed: HashMap<String, HashSet<String>> = HashMap::new();
        match op.list_with(&prefix).await {
            Ok(entries) => {
                for entry in entries {
                    if !entry.metadata().mode().is_file() {
                        continue;
                    }
                    if let Some((filename, tag)) = Self::split_object_name(entry.name()) {
                        listed
                            .entry(filename.to_owned())
                            .or_default()
                            .insert(tag.to_owned());
                    }
                }
            }
            Err(err) if err.kind() == opendal::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        tracing::info!(
            "Found cache entries for {} packages in {prefix}",
            listed.len()
        );
        Ok(Self {
            op,
            prefix,
            listed: RwLock::new(listed),
        })
    }

    fn object_name(filename: &str, tag: &str) -> String {
        format!("{filename}.{tag}.json")
    }

    /// Inverse of [`Self::object_name`]. Tags contain no `.`, so the last two
    /// components are the tag and the `json` extension.
    fn split_object_name(name: &str) -> Option<(&str, &str)> {
        name.strip_suffix(".json")?.rsplit_once('.')
    }

    fn object_path(&self, filename: &str, tag: &str) -> String {
        format!("{}{}", self.prefix, Self::object_name(filename, tag))
    }

    async fn has(&self, filename: &str, tag: &str) -> bool {
        self.listed
            .read()
            .await
            .get(filename)
            .is_some_and(|tags| tags.contains(tag))
    }

    /// Reads the entry for this version of the package. `None` if it does not
    /// exist (anymore) or cannot be used.
    async fn read(&self, filename: &str, tag: &str) -> opendal::Result<Option<CachedPackage>> {
        let path = self.object_path(filename, tag);
        match self.op.read(&path).await {
            Ok(buffer) => match serde_json::from_slice::<CacheEntry>(&buffer.to_bytes()) {
                Ok(entry) => Ok(entry.into_cached()),
                Err(err) => {
                    tracing::warn!("Ignoring unreadable cache object {path}: {err}");
                    Ok(None)
                }
            },
            Err(err) if err.kind() == opendal::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Writes the entry for this version of the package. Another indexer may
    /// have written the identical object already; that is not an error.
    async fn write(
        &self,
        filename: &str,
        tag: &str,
        cached: &CachedPackage,
    ) -> opendal::Result<()> {
        let path = self.object_path(filename, tag);
        let body = serde_json::to_vec(&CacheEntry::new(cached)).map_err(|err| {
            opendal::Error::new(
                opendal::ErrorKind::Unexpected,
                "failed to serialize cache entry",
            )
            .set_source(err)
        })?;
        match self
            .op
            .write_with(&path, body)
            .if_not_exists(true)
            .content_type("application/json")
            .await
        {
            Ok(_) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    opendal::ErrorKind::ConditionNotMatch | opendal::ErrorKind::AlreadyExists
                ) =>
            {
                tracing::trace!("Cache object {path} was written by another indexer");
            }
            Err(err) => return Err(err),
        }
        self.listed
            .write()
            .await
            .entry(filename.to_owned())
            .or_default()
            .insert(tag.to_owned());
        Ok(())
    }

    /// Deletes objects for packages that no longer exist and objects for
    /// superseded versions of packages whose current tag is known.
    async fn prune(
        &self,
        existing: &HashSet<String>,
        current_tags: &HashMap<String, String>,
    ) -> opendal::Result<usize> {
        let stale = {
            let listed = self.listed.read().await;
            listed
                .iter()
                .flat_map(|(filename, tags)| {
                    tags.iter().filter_map(move |tag| {
                        let keep = existing.contains(filename)
                            && current_tags
                                .get(filename)
                                .is_none_or(|current| current == tag);
                        (!keep).then(|| (filename.clone(), tag.clone()))
                    })
                })
                .collect::<Vec<_>>()
        };
        if stale.is_empty() {
            return Ok(0);
        }
        let paths = stale
            .iter()
            .map(|(filename, tag)| self.object_path(filename, tag))
            .collect::<Vec<_>>();
        self.op.delete_iter(paths).await?;
        let mut listed = self.listed.write().await;
        for (filename, tag) in &stale {
            if let Some(tags) = listed.get_mut(filename) {
                tags.remove(tag);
                if tags.is_empty() {
                    listed.remove(filename);
                }
            }
        }
        Ok(stale.len())
    }
}

/// Cache for parsed packages keyed by package filename.
///
/// Thread-safe with `Arc<RwLock<>>` - cheap to clone, all clones share the same
/// storage.
#[derive(Debug, Clone, Default)]
pub struct PackageRecordCache {
    inner: Arc<RwLock<ahash::HashMap<String, CachedPackage>>>,
    store: Option<Arc<ChannelStore>>,
}

/// The package filename of a `<subdir>/<filename>` path.
fn filename_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl PackageRecordCache {
    /// Create a new in-memory cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cache that persists entries in the channel under
    /// `<subdir>/.cache/`, so that later runs and other machines can reuse
    /// them. Lists the existing entries once.
    pub async fn with_channel_store(op: &Operator, subdir: Subdir) -> opendal::Result<Self> {
        let store = ChannelStore::open(op.clone(), subdir).await?;
        Ok(Self {
            inner: Arc::default(),
            store: Some(Arc::new(store)),
        })
    }

    /// Whether entries are persisted in the channel.
    pub fn is_persistent(&self) -> bool {
        self.store.is_some()
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
        let filename = filename_of(path);

        let cached = {
            let guard = self.inner.read().await;
            guard.get(filename).cloned()
        };
        let cached = match cached {
            Some(cached) if cached.metadata.matches(&current) => {
                tracing::debug!("Cache hit for {path}");
                Some(cached)
            }
            Some(_) => {
                tracing::debug!("Cache entry for {path} is stale");
                None
            }
            None => None,
        };

        // Not in memory: look in the channel store.
        let cached = match (cached, &self.store, current.tag()) {
            (Some(cached), _, _) => Some(cached),
            (None, Some(store), Some(tag)) if store.has(filename, &tag).await => {
                let cached = store.read(filename, &tag).await?;
                if let Some(cached) = &cached {
                    tracing::debug!("Cache hit for {path} in the channel store");
                    self.inner
                        .write()
                        .await
                        .insert(filename.to_owned(), cached.clone());
                }
                cached
            }
            _ => None,
        };

        Ok(match cached {
            Some(CachedPackage {
                outcome: CachedOutcome::Parsed(package),
                ..
            }) => CacheResult::Hit(package),
            Some(CachedPackage {
                outcome: CachedOutcome::Broken(error),
                ..
            }) => CacheResult::Broken(error),
            None => {
                tracing::debug!("Cache miss for {path}");
                CacheResult::Miss(current)
            }
        })
    }

    async fn store(&self, path: &str, cached: CachedPackage) {
        let filename = filename_of(path);
        if let Some(store) = &self.store {
            if let Some(tag) = cached.metadata.tag() {
                if let Err(err) = store.write(filename, &tag, &cached).await {
                    tracing::warn!("Failed to write cache entry for {path}: {err}");
                }
            } else {
                tracing::debug!(
                    "Not persisting cache entry for {path}: the backend provided no metadata to identify the file version"
                );
            }
        }
        let mut guard = self.inner.write().await;
        guard.insert(filename.to_owned(), cached);
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

    /// Removes persisted entries that can no longer be used: those of
    /// packages not in `existing_filenames`, and superseded versions of the
    /// packages looked up during this run. Does nothing for an in-memory
    /// cache.
    pub async fn prune(&self, existing_filenames: &HashSet<String>) -> opendal::Result<usize> {
        let Some(store) = &self.store else {
            return Ok(0);
        };
        let current_tags = {
            let guard = self.inner.read().await;
            guard
                .iter()
                .filter_map(|(filename, cached)| {
                    cached.metadata.tag().map(|tag| (filename.clone(), tag))
                })
                .collect::<HashMap<_, _>>()
        };
        let pruned = store.prune(existing_filenames, &current_tags).await?;
        if pruned > 0 {
            tracing::debug!("Pruned {pruned} stale cache objects from {}", store.prefix);
        }
        Ok(pruned)
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
        assert!(!cache.is_persistent());
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

    #[test]
    fn test_tags_have_no_dots_and_round_trip_through_object_names() {
        let quoted_etag = CachedFileMetadata {
            etag: Some("\"abc123-2\"".into()),
            last_modified: None,
            size: None,
        };
        let odd_etag = CachedFileMetadata {
            etag: Some("a/b.c".into()),
            ..quoted_etag.clone()
        };
        let mtime_only = CachedFileMetadata {
            etag: None,
            last_modified: Some(Timestamp::from_second(1_700_000_000).unwrap()),
            size: Some(42),
        };
        for metadata in [quoted_etag, odd_etag, mtime_only] {
            let tag = metadata.tag().unwrap();
            assert!(!tag.contains('.'), "{tag}");
            let filename = "pkg-1.0-0.tar.bz2";
            let name = ChannelStore::object_name(filename, &tag);
            assert_eq!(
                ChannelStore::split_object_name(&name),
                Some((filename, tag.as_str()))
            );
        }
        assert_eq!(CachedFileMetadata::default().tag(), None);
    }
}
