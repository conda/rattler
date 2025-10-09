//! Cache for `PackageRecords` to optimize retry attempts during concurrent indexing.
//!
//! When indexing is retried due to concurrent modifications, we can reuse previously
//! computed `PackageRecords` if the package files themselves haven't changed. This
//! cache validates entries using `ETags` or `last_modified` timestamps to ensure safety.

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

/// Result of a cache lookup operation.
#[derive(Debug)]
pub enum CacheResult {
    /// Cache hit - the record is still valid
    Hit(Box<PackageRecord>),
    /// Cache miss - need to read and parse the file.
    /// Contains the current file metadata for conditional reading.
    Miss {
        /// Current `ETag` of the file (if available)
        etag: Option<String>,
        /// Current last modified time of the file (if available)
        last_modified: Option<DateTime<Utc>>,
    },
}

/// Cache for `PackageRecords` keyed by file path.
///
/// This cache stores computed `PackageRecords` along with their file metadata
/// (`ETag` and `last_modified`). When retrieving from cache, it validates that
/// the file hasn't changed by comparing metadata.
///
/// The cache is designed to be shared across concurrent indexing tasks and
/// to persist across retry attempts within a single indexing operation.
#[derive(Debug, Clone, Default)]
pub struct PackageRecordCache {
    inner: Arc<RwLock<FxHashMap<String, CachedPackage>>>,
}

impl PackageRecordCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a cached record if available and still valid, or return current file metadata.
    ///
    /// This method performs a `stat()` operation to get current file metadata,
    /// then checks if we have a cached entry that matches. If the cached entry
    /// is still valid (`ETag` or `last_modified` matches), returns a cache hit.
    /// Otherwise, returns a cache miss with the current metadata.
    ///
    /// # Arguments
    ///
    /// * `op` - The operator to use for file operations
    /// * `path` - The file path to check (e.g., "noarch/package-0.1.0.tar.bz2")
    ///
    /// # Returns
    ///
    /// Returns `Ok(CacheResult::Hit)` if cache entry is valid, or
    /// `Ok(CacheResult::Miss)` with current metadata if not cached or invalid.
    /// Returns `Err` if the stat operation fails.
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

    /// Insert a computed record into the cache.
    ///
    /// # Arguments
    ///
    /// * `path` - The file path (e.g., "noarch/package-0.1.0.tar.bz2")
    /// * `record` - The computed `PackageRecord`
    /// * `etag` - The file's `ETag` when the record was computed (if available)
    /// * `last_modified` - The file's last modified time when the record was computed (if available)
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
/// This function reads a file using conditional requests (if-match/if-unmodified-since)
/// to ensure we read the same version that was stat'ed. If a `ConditionNotMatch` error
/// occurs (file changed between stat and read), it retries with exponential backoff.
///
/// # Arguments
///
/// * `op` - The operator to use for file operations
/// * `path` - The file path to read
/// * `metadata` - The file metadata to use for conditional reading
///
/// # Returns
///
/// Returns a tuple of (file contents, final metadata) if successful. The final metadata
/// reflects the actual version of the file that was read (which may differ from the
/// initial metadata if retries occurred).
///
/// # Errors
///
/// Returns an error if:
/// - Retries are exhausted due to repeated `ConditionNotMatch` errors
/// - Any non-retryable error occurs (`NotFound`, `PermissionDenied`, etc.)
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
