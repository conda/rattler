//! file storage for passwords.
use anyhow::Result;
use fslock::LockFile;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::authentication_storage::StorageBackend;
use crate::Authentication;

#[derive(Clone, Debug)]
struct FileStorageCache {
    content: BTreeMap<String, Authentication>,
    file_exists: bool,
}

/// A struct that implements storage and access of authentication
/// information backed by a on-disk JSON file
#[derive(Clone, Debug)]
pub struct FileStorage {
    /// The path to the JSON file
    pub path: PathBuf,

    /// The cache of the file storage
    /// This is used to avoid reading the file from disk every time
    /// a credential is accessed
    cache: Arc<RwLock<FileStorageCache>>,
}

/// An error that can occur when accessing the file storage
#[derive(thiserror::Error, Debug)]
pub enum FileStorageError {
    /// An IO error occurred when accessing the file storage
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    /// Failed to lock the file storage file
    #[error("failed to lock file storage file {0}")]
    FailedToLock(String, #[source] std::io::Error),

    /// An error occurred when (de)serializing the credentials
    #[error("JSON error: {0}")]
    JSONError(#[from] serde_json::Error),
}

/// Lock the file storage file for reading and writing. This will block until the lock is
/// acquired.
fn lock_file_storage(path: &Path) -> Result<LockFile, FileStorageError> {
    let path = path.with_extension("lock");
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut lock = fslock::LockFile::open(&path)
        .map_err(|e| FileStorageError::FailedToLock(path.to_string_lossy().into_owned(), e))?;

    // First try to lock the file without block. If we can't immediately get the lock we block and issue a debug message.
    if !lock
        .try_lock_with_pid()
        .map_err(|e| FileStorageError::FailedToLock(path.to_string_lossy().into_owned(), e))?
    {
        tracing::debug!("waiting for lock on {}", path.to_string_lossy());
        lock.lock_with_pid()
            .map_err(|e| FileStorageError::FailedToLock(path.to_string_lossy().into_owned(), e))?;
    }

    Ok(lock)
}

impl FileStorageCache {
    pub fn from_path(path: &Path) -> Result<Self, FileStorageError> {
        let file_exists = path.exists();
        let content = if file_exists {
            let _lock = lock_file_storage(path)?;
            let file = std::fs::File::open(path)?;
            let reader = std::io::BufReader::new(file);
            let content = serde_json::from_reader(reader)?;
            content
        } else {
            BTreeMap::new()
        };

        Ok(Self {
            content,
            file_exists,
        })
    }
}

impl FileStorage {
    /// Create a new file storage with the given path
    pub fn from_path(path: PathBuf) -> Result<Self, FileStorageError> {
        // read the JSON file if it exists, and store it in the cache
        let cache = Arc::new(RwLock::new(FileStorageCache::from_path(&path)?));

        Ok(Self { path, cache })
    }

    /// Create a new file storage with the default path
    pub fn new() -> Result<Self, FileStorageError> {
        let path = dirs::home_dir()
            .unwrap()
            .join(".rattler")
            .join("credentials.json");
        Self::from_path(path)
    }

    /// Updates the cache by reading the JSON file and deserializing it into a `BTreeMap`, or return an empty `BTreeMap` if the
    /// file does not exist
    fn read_json(&self) -> Result<BTreeMap<String, Authentication>, FileStorageError> {
        let new_cache = FileStorageCache::from_path(&self.path)?;
        let mut cache = self.cache.write().unwrap();
        cache.content = new_cache.content;
        cache.file_exists = new_cache.file_exists;
        Ok(cache.content.clone())
    }

    /// Serialize the given `BTreeMap` and write it to the JSON file
    fn write_json(&self, dict: &BTreeMap<String, Authentication>) -> Result<(), FileStorageError> {
        let _lock = lock_file_storage(&self.path)?;

        let file = std::fs::File::create(&self.path)?;
        let writer = std::io::BufWriter::new(file);
        serde_json::to_writer(writer, dict)?;

        // Store the new data in the cache
        let mut cache = self.cache.write().unwrap();
        cache.content = dict.clone();
        cache.file_exists = true;

        Ok(())
    }
}

impl StorageBackend for FileStorage {
    fn store(&self, host: &str, authentication: &crate::Authentication) -> Result<()> {
        let mut dict = self.read_json()?;
        dict.insert(host.to_string(), authentication.clone());
        Ok(self.write_json(&dict)?)
    }

    fn get(&self, host: &str) -> Result<Option<crate::Authentication>> {
        let cache = self.cache.read().unwrap();
        Ok(cache.content.get(host).cloned())
    }

    fn delete(&self, host: &str) -> Result<()> {
        let mut dict = self.read_json()?;
        if dict.remove(host).is_some() {
            Ok(self.write_json(&dict)?)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use std::{fs, io::Write};
    use tempfile::tempdir;

    #[test]
    fn test_file_storage() {
        let file = tempdir().unwrap();
        let path = file.path().join("test.json");

        let storage = FileStorage::from_path(path.clone()).unwrap();

        assert_eq!(storage.get("test").unwrap(), None);

        storage
            .store("test", &Authentication::CondaToken("password".to_string()))
            .unwrap();
        assert_eq!(
            storage.get("test").unwrap(),
            Some(Authentication::CondaToken("password".to_string()))
        );

        storage
            .store(
                "bearer",
                &Authentication::BearerToken("password".to_string()),
            )
            .unwrap();
        storage
            .store(
                "basic",
                &Authentication::BasicHTTP {
                    username: "user".to_string(),
                    password: "password".to_string(),
                },
            )
            .unwrap();

        assert_snapshot!(fs::read_to_string(&path).unwrap());

        storage.delete("test").unwrap();
        assert_eq!(storage.get("test").unwrap(), None);

        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"invalid json").unwrap();

        assert!(FileStorage::from_path(path.clone()).is_err());
    }
}
