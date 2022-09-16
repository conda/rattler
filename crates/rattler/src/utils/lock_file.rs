use std::path::{Path, PathBuf};

/// Provides a cross-platform file locking implementation.
pub struct LockFile {
    path: PathBuf,
    lock: Option<fslock::LockFile>,
}

impl LockFile {
    /// Constructs and locks the file at the specified path. Blocks until the lock file is acquired.
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        // Ensure the directory exists
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        tracing::debug!("acquiring lockfile at '{}'", path.display());

        // Acquire the lock
        let mut lock = fslock::LockFile::open(path)?;

        // Block until the lock can be acquired
        lock.lock()?;

        Ok(LockFile {
            path: path.canonicalize()?,
            lock: Some(lock),
        })
    }

    /// Constructs and locks the file at the specified. Asynchronously waits until the file is
    /// locked.
    pub async fn new_async(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::new(path)).await?
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        drop(self.lock.take());

        // Ignore an error during deletion of the file. If another process acquired the lock this is
        // fine. Worst case an empty file remains.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod test {
    use super::LockFile;
    use std::path::PathBuf;

    #[test]
    fn test_lock_file() {
        let path = PathBuf::from("test.lock");

        // Create the lock file, it should exist
        let lock_file = LockFile::new(&path).unwrap();
        assert!(path.exists());

        // Try opening the lock file again, this should fail
        let mut new_lock_file = fslock::LockFile::open(&path).unwrap();
        assert!(!new_lock_file.try_lock().unwrap());

        // Drop the lock file, the lock file should be gone
        drop(lock_file);
        assert!(!path.exists());
    }
}
