//! Cache for `PackageRecords` to optimize retry attempts during concurrent indexing.
//!
//! When indexing is retried due to concurrent modifications, this cache reuses previously
//! computed [`PackageRecord`]s if the package files haven't changed. Entries are validated
//! using `ETag` or `last_modified` timestamps.
//!
//! Each subdir gets its own cache instance that persists across retry attempts. It works
//! with any `OpenDAL` backend, using conditional reads when supported (S3, HTTP) or simple
//! reads as fallback (filesystem).

use std::{sync::Arc, time::SystemTime};

use chrono::{DateTime, Utc};
use fxhash::FxHashMap;
use opendal::Operator;
use rattler_conda_types::PackageRecord;
use rattler_networking::retry_policies::default_retry_policy;
use retry_policies::{RetryDecision, RetryPolicy};
use tokio::sync::RwLock;

use crate::RepodataFileMetadata;

/// A cached package record with its associated metadata for validation.
#[derive(Debug, Clone)]
struct CachedPackage {
    /// The computed package record
    record: PackageRecord,
    /// The `ETag` when this record was computed (if available)
    etag: Option<String>,
    /// The last modified time when this record was computed (if available)
    last_modified: Option<DateTime<Utc>>,
}

/// Result of a cache lookup operation from [`PackageRecordCache::get_or_stat`].
#[derive(Debug)]
pub enum CacheResult {
    /// Cache hit - the cached record is still valid (metadata matches).
    Hit(Box<PackageRecord>),

    /// Cache miss - need to read and parse the file.
    /// Contains current file metadata for conditional reading.
    Miss {
        /// Current `ETag` of the file (if available)
        etag: Option<String>,
        /// Current last modified time of the file (if available)
        last_modified: Option<DateTime<Utc>>,
    },
}

/// Cache for `PackageRecords` keyed by file path.
///
/// Stores computed [`PackageRecord`]s with their file metadata (`ETag` and `last_modified`).
/// On lookup via [`get_or_stat`], validates entries by comparing current metadata against
/// cached metadata (prefers `ETag`, falls back to `last_modified`).
///
/// Thread-safe with `Arc<RwLock<>>` - cheap to clone, all clones share the same storage.
///
/// [`get_or_stat`]: PackageRecordCache::get_or_stat
#[derive(Debug, Clone, Default)]
pub struct PackageRecordCache {
    inner: Arc<RwLock<FxHashMap<String, CachedPackage>>>,
}

impl PackageRecordCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a cached record if valid, or return current file metadata.
    ///
    /// Performs a `stat()` to get current metadata, then validates any cached entry.
    /// Returns [`CacheResult::Hit`] if metadata matches, or [`CacheResult::Miss`]
    /// with current metadata otherwise.
    pub async fn get_or_stat(&self, op: &Operator, path: &str) -> opendal::Result<CacheResult> {
        // Get current file metadata
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

        let current_etag = metadata.etag().map(str::to_owned);
        let current_last_modified = metadata.last_modified();

        // Check if we have a cached entry
        let cached = {
            let guard = self.inner.read().await;
            guard.get(path).cloned()
        };

        if let Some(cached) = cached {
            // Validate using ETag first (preferred)
            if let (Some(cached_etag), Some(ref current_etag)) = (&cached.etag, &current_etag) {
                if cached_etag == current_etag {
                    tracing::debug!("Cache hit for {} (etag validated)", path);
                    return Ok(CacheResult::Hit(Box::new(cached.record)));
                } else {
                    tracing::debug!(
                        "Cache entry for {} has mismatched etag, treating as miss",
                        path
                    );
                    return Ok(CacheResult::Miss {
                        etag: Some(current_etag.clone()),
                        last_modified: current_last_modified,
                    });
                }
            }

            // Fall back to last_modified comparison
            if let (Some(cached_modified), Some(current_modified)) =
                (cached.last_modified, current_last_modified)
            {
                if cached_modified == current_modified {
                    tracing::debug!("Cache hit for {} (last_modified validated)", path);
                    return Ok(CacheResult::Hit(Box::new(cached.record)));
                } else {
                    tracing::debug!(
                        "Cache entry for {} has mismatched last_modified, treating as miss",
                        path
                    );
                    return Ok(CacheResult::Miss {
                        etag: current_etag,
                        last_modified: current_last_modified,
                    });
                }
            }

            // No way to validate - be conservative and treat as miss
            tracing::debug!(
                "Cache entry for {} cannot be validated (no metadata), treating as miss",
                path
            );
        } else {
            tracing::debug!("Cache miss for {} (not in cache)", path);
        }

        Ok(CacheResult::Miss {
            etag: current_etag,
            last_modified: current_last_modified,
        })
    }

    /// Insert a computed record into the cache with its file metadata.
    ///
    /// Use metadata from the actual read operation (especially important with
    /// [`read_package_with_retry`] which updates metadata during retries).
    pub async fn insert(
        &self,
        path: &str,
        record: PackageRecord,
        etag: Option<String>,
        last_modified: Option<DateTime<Utc>>,
    ) {
        let cached = CachedPackage {
            record,
            etag,
            last_modified,
        };

        let mut guard = self.inner.write().await;
        guard.insert(path.to_string(), cached);
    }
}

/// Read a package file with retry logic for handling concurrent modifications.
///
/// Uses conditional requests (`if-match`/`if-unmodified-since`) to ensure reading the
/// same version that was stat'ed. Retries with exponential backoff if the file changes
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

    // Unit tests for cache structure
    // Integration tests with actual file operations are in tests/integration/

    #[test]
    fn test_cache_creation() {
        let cache = PackageRecordCache::new();
        // Just verify we can create a cache
        assert!(cache.inner.try_read().is_ok());
    }

    #[test]
    fn test_cache_default() {
        let cache = PackageRecordCache::default();
        // Verify default creation works
        assert!(cache.inner.try_read().is_ok());
    }
}
