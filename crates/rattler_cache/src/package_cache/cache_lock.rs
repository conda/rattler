use std::{
    fmt::{Debug, Formatter},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use fs4::fs_std::FileExt;
use parking_lot::Mutex;

use crate::package_cache::PackageCacheError;

/// A lock on the cache entry. As long as this lock is held, no other process is
/// allowed to modify the cache entry. This however, does not guarantee that the
/// contents of the cache is not corrupted by external processes, but it does
/// guarantee that when concurrent processes access the package cache they do
/// not interfere with each other.
pub struct CacheLock {
    pub(super) _lock: CacheRwLock,
    pub(super) revision: u64,
    pub(super) path: PathBuf,
}

impl Debug for CacheLock {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheLock")
            .field("path", &self.path)
            .field("revision", &self.revision)
            .finish()
    }
}

impl CacheLock {
    /// Returns the path to the cache entry on disk.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the revision of the cache entry. This revision indicates the
    /// number of times the cache entry has been updated.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

pub struct CacheRwLock {
    file: Arc<Mutex<std::fs::File>>,
}

impl Drop for CacheRwLock {
    fn drop(&mut self) {
        // Ensure that the lock is released when the lock is dropped.
        let _ = self.file.lock().unlock();
    }
}

impl CacheRwLock {
    pub async fn acquire_read(path: &Path) -> Result<Self, PackageCacheError> {
        let lock_file_path = path.to_path_buf();

        let acquire_lock_fut = simple_spawn_blocking::tokio::run_blocking_task(move || {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .truncate(false)
                .write(true)
                .open(&lock_file_path)
                .map_err(|e| {
                    PackageCacheError::LockError(
                        format!(
                            "failed to open cache lock for reading: '{}'",
                            lock_file_path.display()
                        ),
                        e,
                    )
                })?;

            file.lock_shared().map_err(move |e| {
                PackageCacheError::LockError(
                    format!(
                        "failed to acquire read lock on cache lock file: '{}'",
                        lock_file_path.display()
                    ),
                    e,
                )
            })?;

            Ok(CacheRwLock {
                file: Arc::new(Mutex::new(file)),
            })
        });

        tokio::select!(
            lock = acquire_lock_fut => lock,
            _ = warn_timeout_future(format!(
                "Blocking waiting for file lock on package cache for {}",
                path.file_name()
                    .expect("lock file must have a name")
                    .to_string_lossy()
            )) => unreachable!("warn_timeout_future should never finish")
        )
    }
}

impl CacheRwLock {
    pub async fn acquire_write(path: &Path) -> Result<Self, PackageCacheError> {
        let lock_file_path = path.to_path_buf();
        let acquire_lock_fut = simple_spawn_blocking::tokio::run_blocking_task(move || {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .read(true)
                .open(&lock_file_path)
                .map_err(|e| {
                    PackageCacheError::LockError(
                        format!(
                            "failed to open cache lock for writing: '{}",
                            lock_file_path.display()
                        ),
                        e,
                    )
                })?;

            file.lock_exclusive().map_err(move |e| {
                PackageCacheError::LockError(
                    format!(
                        "failed to acquire write lock on cache lock file: '{}'",
                        lock_file_path.display()
                    ),
                    e,
                )
            })?;

            Ok(CacheRwLock {
                file: Arc::new(Mutex::new(file)),
            })
        });

        tokio::select!(
            lock = acquire_lock_fut => lock,
            _ = warn_timeout_future(format!(
                "Blocking waiting for file lock on package cache for {}",
                path.file_name()
                    .expect("lock file must have a name")
                    .to_string_lossy()
            )) => unreachable!("warn_timeout_future should never finish")
        )
    }
}

impl CacheRwLock {
    pub async fn write_revision(&mut self, revision: u64) -> Result<(), PackageCacheError> {
        let file = self.file.clone();
        simple_spawn_blocking::tokio::run_blocking_task(move || {
            let mut file = file.lock();

            // Ensure we write from the start of the file
            file.rewind().map_err(|e| {
                PackageCacheError::LockError(
                    "failed to rewind cache lock for reading revision".to_string(),
                    e,
                )
            })?;

            // Write the bytes of the revision
            let revision_bytes = revision.to_be_bytes();
            file.write_all(&revision_bytes).map_err(|e| {
                PackageCacheError::LockError(
                    "failed to write revision from cache lock".to_string(),
                    e,
                )
            })?;

            // Ensure all bytes are written to disk
            file.flush().map_err(|e| {
                PackageCacheError::LockError(
                    "failed to flush cache lock after writing revision".to_string(),
                    e,
                )
            })?;

            // Update the length of the file
            file.set_len(revision_bytes.len() as u64).map_err(|e| {
                PackageCacheError::LockError(
                    "failed to truncate cache lock after writing revision".to_string(),
                    e,
                )
            })?;

            Ok(())
        })
        .await
    }
}

impl CacheRwLock {
    pub fn read_revision(&mut self) -> Result<u64, PackageCacheError> {
        let mut buf = [0; 8];
        match self.file.lock().read_exact(&mut buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(0);
            }
            Err(e) => {
                return Err(PackageCacheError::LockError(
                    "failed to read revision from cache lock".to_string(),
                    e,
                ));
            }
        }
        Ok(u64::from_be_bytes(buf))
    }
}

async fn warn_timeout_future(message: String) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        tracing::warn!("{}", &message);
    }
}
