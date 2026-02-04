#![deny(missing_docs)]

//! Content Addressable Store (CAS) for deduplicating file contents across
//! packages.
//!
//! # Architecture
//!
//! The CAS stores files in a directory structure based on their SHA-256 hash:
//! ```text
//! <cas_root>/
//!   <first 2 hex chars>/
//!     <next 2 hex chars>/
//!       <remaining hex chars>
//! ```
//!
//! For example, a file with hash `abc123...` would be stored at:
//! ```text
//! <cas_root>/ab/c1/23...
//! ```
//!
//! # Components
//!
//! - [`SyncWriter`]: A synchronous writer for streaming content to the CAS
//! - [`Writer`]: An async writer for streaming content to the CAS
//! - [`write_sync`]: Writes content from a reader to the CAS (sync)
//! - [`write`]: Writes content from an async reader to the CAS (async)
//!
//! # Usage
//!
//! ```rust,no_run
//! use rattler_cas::SyncWriter;
//! use std::io::Write;
//! use std::path::Path;
//!
//! // Write content to the CAS
//! let cas_root = Path::new("/path/to/cas");
//! let mut w = SyncWriter::create(cas_root).unwrap();
//! w.write_all(b"Hello, world!").unwrap();
//! let hash = w.finish().unwrap();
//!
//! // The file is now stored at the hash-based path
//! println!("Stored with hash {:x}", hash);
//! ```
//!
//! # Deduplication
//!
//! When the same content is written multiple times, the CAS automatically
//! deduplicates:
//! - The hash is computed during writing
//! - If a file with the same hash already exists, the new write is discarded
//! - The existing file path is returned
//!
//! This means extracting multiple packages with identical files only stores
//! each unique file once.

mod sync_writer;
mod writer;

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use rattler_digest::Sha256Hash;
pub use sync_writer::SyncWriter;
pub use writer::Writer;

/// Returns the directory to use for temporary files.
fn temp_dir(root: &Path) -> PathBuf {
    root.join(".tmp")
}

/// Writes the contents of a reader to the CAS and returns the hash of the
/// written file.
///
/// Use `path_for_hash` to get the path of the written file in the CAS.
///
/// # Arguments
///
/// * `root` - The root directory of the CAS store
/// * `reader` - The reader to read content from
pub fn write_sync(root: &Path, reader: &mut impl Read) -> std::io::Result<Sha256Hash> {
    let mut w = SyncWriter::create(root)?;
    std::io::copy(reader, &mut w)?;
    w.finish()
}

/// Writes the contents of an async reader to the CAS and returns the hash of
/// the written file.
///
/// Use `path_for_hash` to get the path of the written file in the CAS.
///
/// # Arguments
///
/// * `root` - The root directory of the CAS store
/// * `reader` - The async reader to read content from
pub async fn write(
    root: &Path,
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
) -> std::io::Result<Sha256Hash> {
    let mut w = Writer::create(root).await?;
    tokio::io::copy(reader, &mut w).await?;
    w.finish().await
}

/// Returns the relative path in the CAS for a given hash. Note that the path
/// might not exist.
///
/// # Arguments
///
/// * `hash` - The hash of the file to look up.
pub fn path_for_hash(hash: &Sha256Hash) -> PathBuf {
    let hash_str = format!("{hash:x}");
    let file_name = format!("{}/{}/{}", &hash_str[0..2], &hash_str[2..4], &hash_str[4..]);
    PathBuf::from(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_digest::Sha256;
    use std::io::{Cursor, Write};

    #[test]
    fn test_path_for_hash() {
        // SHA-256 of empty content
        let hash = rattler_digest::compute_bytes_digest::<Sha256>(b"");
        let path = path_for_hash(&hash);
        assert_eq!(
            path.to_str().unwrap(),
            "e3/b0/c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_write_sync_basic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_root = temp_dir.path();

        let content = b"Hello, CAS!";
        let hash = write_sync(cas_root, &mut Cursor::new(content)).unwrap();

        // Verify the file exists at the expected path
        let file_path = cas_root.join(path_for_hash(&hash));
        assert!(file_path.exists());

        // Verify the content matches
        let stored_content = std::fs::read(&file_path).unwrap();
        assert_eq!(stored_content, content);
    }

    #[test]
    fn test_write_sync_deduplication() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_root = temp_dir.path();

        let content = b"Duplicate content";

        // Write the same content twice
        let hash1 = write_sync(cas_root, &mut Cursor::new(content)).unwrap();
        let hash2 = write_sync(cas_root, &mut Cursor::new(content)).unwrap();

        // Both should return the same hash
        assert_eq!(hash1, hash2);

        // Only one file should exist
        let file_path = cas_root.join(path_for_hash(&hash1));
        assert!(file_path.exists());
    }

    #[test]
    fn test_sync_writer_basic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_root = temp_dir.path();

        let mut writer = SyncWriter::create(cas_root).unwrap();
        writer.write_all(b"Part 1").unwrap();
        writer.write_all(b" Part 2").unwrap();
        let hash = writer.finish().unwrap();

        // Verify the file exists and content is correct
        let file_path = cas_root.join(path_for_hash(&hash));
        let stored_content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(stored_content, "Part 1 Part 2");
    }

    #[test]
    fn test_sync_writer_empty_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_root = temp_dir.path();

        let writer = SyncWriter::create(cas_root).unwrap();
        let hash = writer.finish().unwrap();

        // SHA-256 of empty content
        assert_eq!(
            format!("{hash:x}"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_write_sync_different_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_root = temp_dir.path();

        let hash1 = write_sync(cas_root, &mut Cursor::new(b"Content A")).unwrap();
        let hash2 = write_sync(cas_root, &mut Cursor::new(b"Content B")).unwrap();

        // Different content should have different hashes
        assert_ne!(hash1, hash2);

        // Both files should exist
        assert!(cas_root.join(path_for_hash(&hash1)).exists());
        assert!(cas_root.join(path_for_hash(&hash2)).exists());
    }

    #[tokio::test]
    async fn test_write_async_basic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_root = temp_dir.path();

        let content = b"Hello, async CAS!";
        let mut cursor = Cursor::new(content.to_vec());
        let hash = write(cas_root, &mut cursor).await.unwrap();

        // Verify the file exists at the expected path
        let file_path = cas_root.join(path_for_hash(&hash));
        assert!(file_path.exists());

        // Verify the content matches
        let stored_content = std::fs::read(&file_path).unwrap();
        assert_eq!(stored_content, content);
    }

    #[tokio::test]
    async fn test_write_async_deduplication() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas_root = temp_dir.path();

        let content = b"Async duplicate content";

        // Write the same content twice
        let hash1 = write(cas_root, &mut Cursor::new(content.to_vec()))
            .await
            .unwrap();
        let hash2 = write(cas_root, &mut Cursor::new(content.to_vec()))
            .await
            .unwrap();

        // Both should return the same hash
        assert_eq!(hash1, hash2);
    }

    #[tokio::test]
    async fn test_async_writer_basic() {
        use tokio::io::AsyncWriteExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let cas_root = temp_dir.path();

        let mut writer = Writer::create(cas_root).await.unwrap();
        writer.write_all(b"Async Part 1").await.unwrap();
        writer.write_all(b" Async Part 2").await.unwrap();
        let hash = writer.finish().await.unwrap();

        // Verify the file exists and content is correct
        let file_path = cas_root.join(path_for_hash(&hash));
        let stored_content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(stored_content, "Async Part 1 Async Part 2");
    }
}
