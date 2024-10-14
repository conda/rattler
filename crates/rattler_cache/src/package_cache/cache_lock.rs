use std::{
    fmt::{Debug, Formatter},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use digest::generic_array::GenericArray;
use fs4::fs_std::FileExt;
use parking_lot::Mutex;
use rattler_digest::Sha256Hash;

use crate::package_cache::PackageCacheError;

/// A lock on the cache entry. As long as this lock is held, no other process is
/// allowed to modify the cache entry. This however, does not guarantee that the
/// contents of the cache is not corrupted by external processes, but it does
/// guarantee that when concurrent processes access the package cache they do
/// not interfere with each other.
pub struct CacheLock {
    pub(super) _lock: CacheRwLock,
    pub(super) revision: u64,
    pub(super) sha256: Option<Sha256Hash>,
    pub(super) path: PathBuf,
}

impl Debug for CacheLock {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheLock")
            .field("path", &self.path)
            .field("revision", &self.revision)
            .field("sha256", &self.sha256)
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
    pub async fn write_revision_and_sha(
        &mut self,
        revision: u64,
        sha256: Option<&Sha256Hash>,
    ) -> Result<(), PackageCacheError> {
        let file = self.file.clone();
        let sha256 = sha256.cloned();
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

            // Write the bytes of the sha256 hash
            let sha_bytes = if let Some(sha) = sha256 {
                let len = sha.len();
                let sha = &sha[..];
                file.write_all(sha).map_err(|e| {
                    PackageCacheError::LockError(
                        "failed to write sha256 from cache lock".to_string(),
                        e,
                    )
                })?;
                len
            } else {
                0
            };

            // Ensure all bytes are written to disk
            file.flush().map_err(|e| {
                PackageCacheError::LockError(
                    "failed to flush cache lock after writing revision".to_string(),
                    e,
                )
            })?;

            // Update the length of the file
            let file_length = revision_bytes.len() + sha_bytes;
            file.set_len(file_length as u64).map_err(|e| {
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
    /// Reads the revision from the cache lock file.
    pub fn read_revision(&mut self) -> Result<u64, PackageCacheError> {
        let mut file = self.file.lock();
        file.rewind().map_err(|e| {
            PackageCacheError::LockError(
                "failed to rewind cache lock for reading revision".to_string(),
                e,
            )
        })?;
        let mut buf = [0; 8];
        match file.read_exact(&mut buf) {
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

    /// Reads the sha256 hash from the cache lock file.
    pub fn read_sha256(&mut self) -> Result<Option<Sha256Hash>, PackageCacheError> {
        const SHA256_LEN: usize = 32;
        const REVISION_LEN: u64 = 8;
        let mut file = self.file.lock();
        file.rewind().map_err(|e| {
            PackageCacheError::LockError(
                "failed to rewind cache lock for reading sha256".to_string(),
                e,
            )
        })?;
        let mut buf = [0; SHA256_LEN];
        let _ = file.seek(SeekFrom::Start(REVISION_LEN)).map_err(|e| {
            PackageCacheError::LockError("failed to seek to sha256 in cache lock".to_string(), e)
        })?;
        match file.read_exact(&mut buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(None);
            }
            Err(e) => {
                return Err(PackageCacheError::LockError(
                    "failed to read sha256 from cache lock".to_string(),
                    e,
                ));
            }
        }
        Ok(Some(GenericArray::clone_from_slice(&buf)))
    }
}

async fn warn_timeout_future(message: String) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        tracing::warn!("{}", &message);
    }
}

#[cfg(test)]
mod tests {
    use rattler_digest::{parse_digest_from_hex, Sha256};

    use super::CacheRwLock;

    #[tokio::test]
    async fn cache_lock_serialize_deserialize() {
        // Temporarily create a lock file and write a revision and sha to it
        let temp_dir = tempfile::tempdir().unwrap();
        let lock_file = temp_dir.path().join("foo.lock");
        // Acquire a write lock on the file
        let mut lock = CacheRwLock::acquire_write(&lock_file).await.unwrap();
        // Write a revision and sha to the lock file
        let sha = parse_digest_from_hex::<Sha256>(
            "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8",
        );
        lock.write_revision_and_sha(1, sha.as_ref()).await.unwrap();
        // Read back the revision and sha from the lock file
        let revision = lock.read_revision().unwrap();
        assert_eq!(revision, 1);
        let read_sha = lock.read_sha256().unwrap();
        assert_eq!(sha, read_sha);
    }
}
