#[cfg(test)]
mod tests {
    use super::super::etag_memory_backend::*;
    use opendal::{ErrorKind, Operator};

    /// Validates basic read/write operations work correctly.
    #[tokio::test]
    async fn test_basic_read_write() {
        let op = Operator::new(ETagMemoryBuilder::default()).unwrap();

        // Write a file
        op.write("test.txt", "hello world").await.unwrap();

        // Read it back
        let data = op.read("test.txt").await.unwrap();
        assert_eq!(data.to_bytes(), "hello world");
    }

    /// Validates that `ETag`s are generated and change when file content changes.
    #[tokio::test]
    async fn test_etag_generation() {
        let op = Operator::new(ETagMemoryBuilder::default()).unwrap();

        op.write("test.txt", "hello").await.unwrap();
        let meta = op.stat("test.txt").await.unwrap();

        assert!(meta.etag().is_some());
        let etag1 = meta.etag().unwrap();

        // Write different content
        op.write("test.txt", "world").await.unwrap();
        let meta2 = op.stat("test.txt").await.unwrap();
        let etag2 = meta2.etag().unwrap();

        // ETags should be different
        assert_ne!(etag1, etag2);
    }

    /// Validates that conditional reads with if-match work correctly.
    ///
    /// Tests that reads succeed when `ETag` matches and fail with
    /// `ConditionNotMatch` when it doesn't.
    #[tokio::test]
    async fn test_conditional_read_with_if_match() {
        let op = Operator::new(ETagMemoryBuilder::default()).unwrap();

        op.write("test.txt", "hello").await.unwrap();
        let meta = op.stat("test.txt").await.unwrap();
        let etag = meta.etag().unwrap();

        // Read with matching ETag should succeed
        let result = op.read_with("test.txt").if_match(etag).await;
        assert!(result.is_ok());

        // Read with wrong ETag should fail
        let result = op.read_with("test.txt").if_match("\"wrong\"").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), ErrorKind::ConditionNotMatch);
    }

    /// Validates that conditional writes with if-match work correctly.
    ///
    /// Tests that writes succeed when `ETag` matches and fail with
    /// `ConditionNotMatch` when it doesn't.
    #[tokio::test]
    async fn test_conditional_write_with_if_match() {
        let op = Operator::new(ETagMemoryBuilder::default()).unwrap();

        op.write("test.txt", "v1").await.unwrap();
        let meta = op.stat("test.txt").await.unwrap();
        let etag = meta.etag().unwrap();

        // Write with matching ETag should succeed
        let result = op.write_with("test.txt", "v2").if_match(etag).await;
        assert!(result.is_ok());

        // Write with wrong ETag should fail
        let result = op.write_with("test.txt", "v3").if_match("\"wrong\"").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), ErrorKind::ConditionNotMatch);
    }

    /// Validates that if-none-match with "*" enforces create-only semantics.
    ///
    /// Tests that the first write succeeds but subsequent writes with
    /// if-none-match="*" fail.
    #[tokio::test]
    async fn test_if_none_match_create_only() {
        let op = Operator::new(ETagMemoryBuilder::default()).unwrap();

        // First write with if_none_match should succeed
        let result = op.write_with("test.txt", "v1").if_none_match("*").await;
        assert!(result.is_ok());

        // Second write with if_none_match should fail
        let result = op.write_with("test.txt", "v2").if_none_match("*").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), ErrorKind::ConditionNotMatch);
    }
}
