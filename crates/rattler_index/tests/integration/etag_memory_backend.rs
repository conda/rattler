// Custom in-memory opendal backend with ETag and conditional request support
// for testing
//
// This backend provides an in-memory storage system that properly implements
// ETags and conditional requests (if_match, if_none_match, if_unmodified_since)
// similar to how S3 behaves. This allows us to test race condition handling
// without needing actual S3.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use opendal::{
    raw::*, Buffer, Builder, Capability, EntryMode, Error, ErrorKind, Metadata, Operator, Result,
    Scheme,
};
use tokio::sync::RwLock;

const SCHEME_NAME: &str = "etag-memory";

// Small helpers to reduce duplication in conditional checks
// Note: ctx is used to disambiguate the operation in error messages (e.g. " on
// read")
#[inline]
fn check_if_match(current: &str, cond: Option<&str>, ctx: &str) -> Result<()> {
    if let Some(if_match) = cond {
        if if_match != current {
            return Err(Error::new(
                ErrorKind::ConditionNotMatch,
                format!("ETag mismatch{ctx}: expected {if_match}, got {current}"),
            ));
        }
    }
    Ok(())
}

#[inline]
fn check_if_none_match(current: &str, cond: Option<&str>, ctx: &str) -> Result<()> {
    if let Some(if_none_match) = cond {
        if if_none_match == "*" || if_none_match == current {
            return Err(Error::new(
                ErrorKind::ConditionNotMatch,
                format!("if_none_match condition failed{ctx}"),
            ));
        }
    }
    Ok(())
}

#[inline]
fn check_if_unmodified_since(
    last_modified: DateTime<Utc>,
    cond: Option<DateTime<Utc>>,
) -> Result<()> {
    if let Some(if_unmodified_since) = cond {
        if last_modified > if_unmodified_since {
            return Err(Error::new(
                ErrorKind::ConditionNotMatch,
                "file was modified after if_unmodified_since",
            ));
        }
    }
    Ok(())
}

/// Entry stored in the `ETag` memory backend
#[derive(Clone, Debug)]
struct FileEntry {
    data: Bytes,
    etag: String,
    last_modified: DateTime<Utc>,
    content_length: u64,
}

impl FileEntry {
    fn new(data: Bytes) -> Self {
        let etag = Self::compute_etag(&data);
        Self {
            content_length: data.len() as u64,
            data,
            etag,
            last_modified: Utc::now(),
        }
    }

    /// Compute `ETag` as MD5 hex (mimics S3 behavior)
    fn compute_etag(data: &[u8]) -> String {
        format!("\"{:x}\"", md5::compute(data))
    }
}

use std::{future::Future, pin::Pin};

/// Operations that can be intercepted
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    BeforeStat,
    AfterStat,
    BeforeRead,
    AfterRead,
    BeforeWrite,
    AfterWrite,
}

/// Callback function for test synchronization
/// Returns a future that must complete before/after the operation proceeds
pub type TestCallback =
    Arc<dyn Fn(&str, Operation) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Test hooks for synchronizing operations in tests
#[derive(Clone)]
pub struct TestHooks {
    /// Callback invoked for operations
    pub on_operation: TestCallback,
}

impl std::fmt::Debug for TestHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestHooks").finish()
    }
}

impl Default for TestHooks {
    fn default() -> Self {
        Self {
            on_operation: Arc::new(|_, _| Box::pin(async {})),
        }
    }
}

/// In-memory storage with `ETag` support
#[derive(Clone, Debug)]
pub struct ETagMemoryBackend {
    storage: Arc<RwLock<HashMap<String, Arc<RwLock<FileEntry>>>>>,
    directories: Arc<RwLock<HashSet<String>>>,
    test_hooks: Option<TestHooks>,
}

impl ETagMemoryBackend {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
            directories: Arc::new(RwLock::new(HashSet::new())),
            test_hooks: None,
        }
    }

    pub fn with_test_hooks(hooks: TestHooks) -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
            directories: Arc::new(RwLock::new(HashSet::new())),
            test_hooks: Some(hooks),
        }
    }
}

/// Builder for `ETag` memory backend
#[derive(Default, Debug)]
pub struct ETagMemoryBuilder {
    test_hooks: Option<TestHooks>,
}

impl ETagMemoryBuilder {
    pub fn with_test_hooks(mut self, hooks: TestHooks) -> Self {
        self.test_hooks = Some(hooks);
        self
    }
}

impl Builder for ETagMemoryBuilder {
    const SCHEME: Scheme = Scheme::Custom(SCHEME_NAME);
    type Config = ();

    fn build(self) -> Result<impl Access> {
        if let Some(hooks) = self.test_hooks {
            Ok(ETagMemoryBackend::with_test_hooks(hooks))
        } else {
            Ok(ETagMemoryBackend::new())
        }
    }
}

impl Access for ETagMemoryBackend {
    type Reader = Buffer;
    type Writer = ETagMemoryWriter;
    type Lister = oio::HierarchyLister<ETagMemoryLister>;
    type Deleter = oio::OneShotDeleter<ETagMemoryDeleter>;

    fn info(&self) -> Arc<AccessorInfo> {
        let info = AccessorInfo::default();
        info.set_scheme(Scheme::Custom(SCHEME_NAME))
            .set_root("/")
            .set_native_capability(Capability {
                stat: true,
                stat_with_if_match: true,
                stat_with_if_none_match: true,
                read: true,
                read_with_if_match: true,
                read_with_if_none_match: true,
                write: true,
                write_can_empty: true,
                write_with_if_match: true,
                write_with_if_none_match: true,
                write_with_if_not_exists: true,
                delete: true,
                list: true,
                create_dir: true,
                ..Default::default()
            });

        info.into()
    }

    async fn stat(&self, path: &str, args: OpStat) -> Result<RpStat> {
        // Test hook: call callback before operation
        if let Some(hooks) = &self.test_hooks {
            (hooks.on_operation)(path, Operation::BeforeStat).await;
        }

        let metadata = {
            let storage = self.storage.read().await;
            let entry_lock = if let Some(entry_lock) = storage.get(path).cloned() {
                entry_lock
            } else {
                if let Some(hooks) = &self.test_hooks {
                    (hooks.on_operation)(path, Operation::AfterStat).await;
                }
                return Err(Error::new(
                    ErrorKind::NotFound,
                    format!("file not found: {path}"),
                ));
            };
            drop(storage);

            let entry = entry_lock.read().await;
            // Check conditions
            check_if_match(&entry.etag, args.if_match(), "")?;
            check_if_none_match(&entry.etag, args.if_none_match(), "")?;

            Metadata::new(EntryMode::FILE)
                .with_etag(entry.etag.clone())
                .with_last_modified(entry.last_modified)
                .with_content_length(entry.content_length)
        }; // entry lock is dropped here

        // Test hook: call callback after operation
        if let Some(hooks) = &self.test_hooks {
            (hooks.on_operation)(path, Operation::AfterStat).await;
        }

        Ok(RpStat::new(metadata))
    }

    async fn read(&self, path: &str, args: OpRead) -> Result<(RpRead, Self::Reader)> {
        // Test hook: call callback before operation
        if let Some(hooks) = &self.test_hooks {
            (hooks.on_operation)(path, Operation::BeforeRead).await;
        }

        let (etag, last_modified, data) = {
            let storage = self.storage.read().await;
            let entry_lock = storage
                .get(path)
                .ok_or_else(|| Error::new(ErrorKind::NotFound, "file not found"))?
                .clone();
            drop(storage);

            let entry = entry_lock.read().await;
            (entry.etag.clone(), entry.last_modified, entry.data.clone())
        }; // entry lock is dropped here

        // Check conditions
        check_if_match(&etag, args.if_match(), " on read")?;
        check_if_none_match(&etag, args.if_none_match(), " on read")?;
        check_if_unmodified_since(last_modified, args.if_unmodified_since())?;

        // Test hook: call callback after operation
        if let Some(hooks) = &self.test_hooks {
            (hooks.on_operation)(path, Operation::AfterRead).await;
        }

        // Note: metadata is available via stat(), not through RpRead
        Ok((RpRead::new(), Buffer::from(data)))
    }

    async fn write(&self, path: &str, args: OpWrite) -> Result<(RpWrite, Self::Writer)> {
        // Test hook: call callback before operation
        if let Some(hooks) = &self.test_hooks {
            (hooks.on_operation)(path, Operation::BeforeWrite).await;
        }

        // Get or create the entry lock
        let entry_lock = {
            let mut storage = self.storage.write().await;
            if let Some(entry_lock) = storage.get(path) {
                let entry_lock = entry_lock.clone();
                drop(storage);

                // Acquire owned write lock - this will block if another write is in progress
                let entry = entry_lock.write_owned().await;
                check_if_match(&entry.etag, args.if_match(), " on write")?;

                // Check if_none_match for create-only semantics
                if let Some(if_none_match) = args.if_none_match() {
                    if if_none_match == "*" {
                        return Err(Error::new(
                            ErrorKind::ConditionNotMatch,
                            "if_none_match: file already exists",
                        ));
                    }
                }

                // Check if_not_exists for create-only semantics
                if args.if_not_exists() {
                    return Err(Error::new(
                        ErrorKind::ConditionNotMatch,
                        "if_not_exists: file already exists",
                    ));
                }

                entry
            } else {
                // File doesn't exist - check if_match (should fail)
                if args.if_match().is_some() {
                    return Err(Error::new(
                        ErrorKind::ConditionNotMatch,
                        "if_match specified but file doesn't exist",
                    ));
                }

                // Create new entry lock and acquire owned write guard
                let entry = Arc::new(RwLock::new(FileEntry::new(Bytes::new())));
                storage.insert(path.to_owned(), entry.clone());
                drop(storage);

                entry.write_owned().await
            }
        };

        Ok((
            RpWrite::new(),
            ETagMemoryWriter {
                path: path.to_owned(),
                entry_lock,
                buffer: oio::QueueBuf::new(),
                test_hooks: self.test_hooks.clone(),
            },
        ))
    }

    async fn create_dir(&self, path: &str, _args: OpCreateDir) -> Result<RpCreateDir> {
        let mut directories = self.directories.write().await;
        directories.insert(path.trim_end_matches('/').to_owned());
        Ok(RpCreateDir::default())
    }

    async fn delete(&self) -> Result<(RpDelete, Self::Deleter)> {
        Ok((
            RpDelete::default(),
            oio::OneShotDeleter::new(ETagMemoryDeleter {
                storage: self.storage.clone(),
            }),
        ))
    }

    async fn list(&self, path: &str, _args: OpList) -> Result<(RpList, Self::Lister)> {
        let storage = self.storage.read().await;
        let directories = self.directories.read().await;

        let prefix = if path == "/" || path.is_empty() {
            String::new()
        } else {
            path.trim_end_matches('/').to_owned()
        };
        let prefix_slash = if prefix.is_empty() {
            None
        } else {
            Some(format!("{prefix}/"))
        };

        let mut entries: Vec<(String, bool)> = Vec::new();
        let mut seen = HashSet::new();

        // Add direct child directories
        for dir in directories.iter() {
            if prefix.is_empty() {
                // List root level directories
                if let Some(first) = dir.split('/').next() {
                    if !first.is_empty() && seen.insert(first) {
                        entries.push((format!("{first}/"), true));
                    }
                }
            } else if let Some(ps) = &prefix_slash {
                if let Some(stripped) = dir.strip_prefix(ps) {
                    // List subdirectories under prefix
                    if let Some(first) = stripped.split('/').next() {
                        if !first.is_empty() && seen.insert(first) {
                            entries.push((format!("{first}/"), true));
                        }
                    }
                }
            }
        }

        // Add direct child files
        for key in storage.keys() {
            if prefix.is_empty() {
                // List root level files (no / in path)
                if !key.contains('/') {
                    entries.push((key.clone(), false));
                }
            } else if let Some(ps) = &prefix_slash {
                if let Some(stripped) = key.strip_prefix(ps) {
                    // List files directly under prefix (no further / in stripped path)
                    if !stripped.contains('/') {
                        entries.push((stripped.to_owned(), false));
                    }
                }
            }
        }

        drop(storage);
        drop(directories);

        Ok((
            RpList::default(),
            oio::HierarchyLister::new(ETagMemoryLister { entries, index: 0 }, "/", false),
        ))
    }
}

/// Writer that stores data and generates `ETag` on completion
pub struct ETagMemoryWriter {
    entry_lock: tokio::sync::OwnedRwLockWriteGuard<FileEntry>,
    buffer: oio::QueueBuf,
    path: String,
    test_hooks: Option<TestHooks>,
}

impl oio::Write for ETagMemoryWriter {
    async fn write(&mut self, bs: Buffer) -> Result<()> {
        self.buffer.push(bs);
        Ok(())
    }

    async fn close(&mut self) -> Result<Metadata> {
        let data = self.buffer.clone().collect().to_bytes();
        let new_entry = FileEntry::new(data);

        let metadata = Metadata::new(EntryMode::FILE)
            .with_etag(new_entry.etag.clone())
            .with_last_modified(new_entry.last_modified)
            .with_content_length(new_entry.content_length);

        *self.entry_lock = new_entry;

        // Test hook: call callback after operation
        if let Some(hooks) = &self.test_hooks {
            (hooks.on_operation)(&self.path, Operation::AfterWrite).await;
        }

        Ok(metadata)
    }

    async fn abort(&mut self) -> Result<()> {
        self.buffer.clear();
        Ok(())
    }
}

/// Deleter for `ETag` memory backend
pub struct ETagMemoryDeleter {
    storage: Arc<RwLock<HashMap<String, Arc<RwLock<FileEntry>>>>>,
}

impl oio::OneShotDelete for ETagMemoryDeleter {
    async fn delete_once(&self, path: String, _args: OpDelete) -> Result<()> {
        let mut storage = self.storage.write().await;
        storage.remove(&path);
        Ok(())
    }
}

/// Lister for `ETag` memory backend
pub struct ETagMemoryLister {
    entries: Vec<(String, bool)>, // (path, is_dir)
    index: usize,
}

impl oio::List for ETagMemoryLister {
    async fn next(&mut self) -> Result<Option<oio::Entry>> {
        if self.index >= self.entries.len() {
            return Ok(None);
        }

        let (path, is_dir) = self.entries[self.index].clone();
        self.index += 1;

        let mode = if is_dir {
            EntryMode::DIR
        } else {
            EntryMode::FILE
        };
        let entry = oio::Entry::new(&path, Metadata::new(mode));
        Ok(Some(entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_read_write() {
        let op = Operator::new(ETagMemoryBuilder::default())
            .unwrap()
            .finish();

        // Write a file
        op.write("test.txt", "hello world").await.unwrap();

        // Read it back
        let data = op.read("test.txt").await.unwrap();
        assert_eq!(data.to_bytes(), "hello world");
    }

    #[tokio::test]
    async fn test_etag_generation() {
        let op = Operator::new(ETagMemoryBuilder::default())
            .unwrap()
            .finish();

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

    #[tokio::test]
    async fn test_conditional_read_with_if_match() {
        let op = Operator::new(ETagMemoryBuilder::default())
            .unwrap()
            .finish();

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

    #[tokio::test]
    async fn test_conditional_write_with_if_match() {
        let op = Operator::new(ETagMemoryBuilder::default())
            .unwrap()
            .finish();

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

    #[tokio::test]
    async fn test_if_none_match_create_only() {
        let op = Operator::new(ETagMemoryBuilder::default())
            .unwrap()
            .finish();

        // First write with if_none_match should succeed
        let result = op.write_with("test.txt", "v1").if_none_match("*").await;
        assert!(result.is_ok());

        // Second write with if_none_match should fail
        let result = op.write_with("test.txt", "v2").if_none_match("*").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), ErrorKind::ConditionNotMatch);
    }
}
