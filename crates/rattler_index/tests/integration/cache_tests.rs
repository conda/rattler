//! Integration tests for the `PackageRecordCache` and `read_package_with_retry`

use futures::TryStreamExt;
use opendal::Operator;
use rattler_index::{
    RepodataFileMetadata,
    cache::{open_package_with_retry, read_package_with_retry},
};

use super::etag_memory_backend::ETagMemoryBuilder;

/// Validates that `read_package_with_retry` successfully retries when the file
/// changes between stat and read operations.
///
/// This test:
/// 1. Writes a file with initial content (v1)
/// 2. Gets the `ETag` for v1
/// 3. Updates the file to v2 (changes `ETag`)
/// 4. Calls `read_package_with_retry` with the old v1 metadata
/// 5. Verifies the function retries and succeeds with v2 content and metadata
#[tokio::test]
async fn test_read_package_with_retry_success_after_etag_change() {
    let op = Operator::new(ETagMemoryBuilder::default())
        .unwrap()
        .finish();

    let path = "test/package.tar.bz2";
    let content_v1 = "version 1 content";
    let content_v2 = "version 2 content";

    // Write initial version
    op.write(path, content_v1).await.unwrap();

    // Get metadata for v1
    let metadata_v1 = op.stat(path).await.unwrap();
    let etag_v1 = metadata_v1.etag().map(str::to_owned);

    // Update the file to version 2
    op.write(path, content_v2).await.unwrap();

    // Get the new metadata (for verification later)
    let metadata_v2 = op.stat(path).await.unwrap();
    let etag_v2 = metadata_v2.etag().map(str::to_owned);

    // Verify ETags are different
    assert_ne!(etag_v1, etag_v2);

    // Try to read with old metadata - should retry and succeed with new version
    let old_metadata = RepodataFileMetadata {
        etag: etag_v1,
        last_modified: metadata_v1.last_modified(),
        file_existed: true,
        precondition_checks: rattler_index::PreconditionChecks::Enabled,
    };

    let result = read_package_with_retry(&op, path, old_metadata).await;

    // Should succeed after retry
    assert!(result.is_ok());
    let (buffer, final_metadata) = result.unwrap();

    // Verify we got the new version
    assert_eq!(buffer.to_bytes(), content_v2.as_bytes());

    // Verify the returned metadata matches v2
    assert_eq!(final_metadata.etag, etag_v2);
}

/// Validates that `read_package_with_retry` propagates non-retryable errors
/// immediately without attempting retries.
///
/// This test verifies that errors like `NotFound` are not retried and are
/// instead propagated immediately to the caller.
#[tokio::test]
async fn test_read_package_with_retry_propagates_other_errors() {
    let op = Operator::new(ETagMemoryBuilder::default())
        .unwrap()
        .finish();

    let path = "nonexistent/file.tar.bz2";

    // Try to read a non-existent file
    let metadata = RepodataFileMetadata {
        etag: Some("fake-etag".to_string()),
        last_modified: None,
        file_existed: true,
        precondition_checks: rattler_index::PreconditionChecks::Enabled,
    };

    let result = read_package_with_retry(&op, path, metadata).await;

    // Should fail with NotFound error (not retry)
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), opendal::ErrorKind::NotFound);
}

/// Validates that `open_package_with_retry` retries when the file changed
/// since it was stated, just like `read_package_with_retry`.
///
/// `OpenDAL` opens streams lazily, so this checks that the precondition
/// failure is still caught when the stream is opened rather than while it is
/// being read.
#[tokio::test]
async fn test_open_package_with_retry_success_after_etag_change() {
    let op = Operator::new(ETagMemoryBuilder::default())
        .unwrap()
        .finish();

    let path = "test/package.tar.bz2";
    let content_v1 = "version 1 content";
    let content_v2 = "version 2 content";

    op.write(path, content_v1).await.unwrap();
    let metadata_v1 = op.stat(path).await.unwrap();
    op.write(path, content_v2).await.unwrap();
    let etag_v2 = op.stat(path).await.unwrap().etag().map(str::to_owned);

    let old_metadata = RepodataFileMetadata {
        etag: metadata_v1.etag().map(str::to_owned),
        last_modified: metadata_v1.last_modified(),
        file_existed: true,
        precondition_checks: rattler_index::PreconditionChecks::Enabled,
    };

    let (stream, final_metadata) = open_package_with_retry(&op, path, old_metadata)
        .await
        .unwrap();
    let content: Vec<u8> = stream
        .map_ok(|chunk| chunk.to_vec())
        .try_concat()
        .await
        .unwrap();

    assert_eq!(content, content_v2.as_bytes());
    assert_eq!(final_metadata.etag, etag_v2);
}
