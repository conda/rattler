use chrono::{TimeZone, Utc};
use opendal::Operator;

use crate::RepodataFileMetadata;

/// Reads a file with conditional checks based on provided metadata.
///
/// This function reads a file and validates that it hasn't been modified since
/// the metadata was collected. If the file has been modified (`ETag` or
/// `last_modified` doesn't match), it returns a `ConditionNotMatch` error.
///
/// If metadata has no `ETag` or `last_modified` (either because the file didn't exist,
/// precondition checks are disabled, or the backend doesn't support it), the file
/// is read without conditional checks.
///
/// # Parameters
/// - `op`: A reference to the `Operator`, which facilitates file system
///   operations.
/// - `path`: A string slice that specifies the file path to read.
/// - `metadata`: The metadata to use for conditional read validation.
///
/// # Returns
/// Returns `Ok(Buffer)` if the file is successfully read and conditions match.
/// Returns `Err` with `ConditionNotMatch` if the file was modified since
/// metadata collection.
pub async fn read_with_metadata_check(
    op: &Operator,
    path: &str,
    metadata: &RepodataFileMetadata,
) -> opendal::Result<opendal::Buffer> {
    let mut reader = op.read_with(path);

    // Only apply precondition checks if they're enabled
    if metadata.precondition_checks.is_enabled() {
        // Prefer ETag for precise change detection
        if let Some(etag) = &metadata.etag {
            reader = reader.if_match(etag);
        }
        // Fall back to last_modified timestamp
        else if let Some(last_modified) = metadata.last_modified {
            // Convert jiff::Timestamp to chrono::DateTime<Utc> for opendal
            let chrono_dt = Utc.timestamp_millis_opt(last_modified.as_millisecond()).single();
            if let Some(dt) = chrono_dt {
                reader = reader.if_unmodified_since(dt);
            }
        }
        // else: no metadata available, proceed without conditions
    }

    reader.await
}

/// Writes a file with conditional checks based on provided metadata.
///
/// This function writes a file and validates that it hasn't been modified since
/// the metadata was collected. If the file has been modified (`ETag` doesn't
/// match), it returns a `ConditionNotMatch` error.
///
/// When the file didn't exist during metadata collection (etag is None), this
/// function uses `if_not_exists` to ensure the file still doesn't exist,
/// preventing race conditions where another process creates it first.
///
/// If metadata has no etag (either because the file didn't exist, precondition
/// checks are disabled, or the backend doesn't support it), the file is written
/// without conditional checks.
///
/// # Parameters
/// - `op`: A reference to the `Operator`, which facilitates file system
///   operations.
/// - `path`: A string slice that specifies the file path to write.
/// - `data`: The data to write to the file.
/// - `metadata`: The metadata to use for conditional write validation.
/// - `cache_control`: Optional cache control header value to set (for S3 metadata).
///
/// # Returns
/// Returns `Ok(())` if the file is successfully written and conditions match.
/// Returns `Err` with `ConditionNotMatch` if the file was modified since
/// metadata collection or if the file was created when it shouldn't exist.
pub async fn write_with_metadata_check(
    op: &Operator,
    path: &str,
    data: Vec<u8>,
    metadata: &RepodataFileMetadata,
    cache_control: Option<&str>,
) -> opendal::Result<opendal::Metadata> {
    let mut writer = op.write_with(path, data);

    // Only apply precondition checks if they're enabled
    if metadata.precondition_checks.is_enabled() {
        if let Some(etag) = &metadata.etag {
            // File existed - verify it hasn't changed
            writer = writer.if_match(etag);
        } else if !metadata.file_existed {
            // File didn't exist - ensure it still doesn't (prevents race conditions)
            writer = writer.if_not_exists(true);
        }
        // else: file existed but no etag support, proceed without conditions
    }

    // Set cache control header if provided
    if let Some(cache_control_value) = cache_control {
        writer = writer.cache_control(cache_control_value);
    }

    writer.await
}
