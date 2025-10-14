use opendal::Operator;

use crate::RepodataFileMetadata;

/// Reads a file with conditional checks based on provided metadata.
///
/// This function reads a file and validates that it hasn't been modified since
/// the metadata was collected. If the file has been modified (`ETag` or
/// last-modified doesn't match), it returns a `ConditionNotMatch` error.
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
    let reader = op.read_with(path);
    let reader = if let Some(etag) = &metadata.etag {
        reader.if_match(etag)
    } else if let Some(last_modified) = metadata.last_modified {
        reader.if_unmodified_since(last_modified)
    } else {
        // If no metadata available, just read without conditions
        reader
    };
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
/// # Parameters
/// - `op`: A reference to the `Operator`, which facilitates file system
///   operations.
/// - `path`: A string slice that specifies the file path to write.
/// - `data`: The data to write to the file.
/// - `metadata`: The metadata to use for conditional write validation.
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
) -> opendal::Result<()> {
    let writer = op.write_with(path, data);
    let writer = if let Some(etag) = &metadata.etag {
        // File existed - verify it hasn't changed
        writer.if_match(etag)
    } else {
        // File didn't exist - ensure it still doesn't exist
        writer.if_not_exists(true)
    };
    writer.await?;
    Ok(())
}
