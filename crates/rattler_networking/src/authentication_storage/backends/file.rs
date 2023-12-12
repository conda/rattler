//! file storage for passwords.
use anyhow::Result;
use fslock::LockFile;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::{path::PathBuf, sync::Mutex};

use crate::authentication_storage::StorageBackend;
use crate::Authentication;

/// A struct that implements storage and access of authentication
/// information backed by a on-disk JSON file
#[derive(Clone, Debug)]
pub struct FileStorage {
    /// The path to the JSON file
    pub path: PathBuf,
}

/// An error that can occur when accessing the file storage
#[derive(thiserror::Error, Debug)]
pub enum FileStorageError {
    /// An IO error occurred when accessing the file storage
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    /// Failed to lock the file storage file
    #[error("failed to lock file storage file {0}.")]
    FailedToLock(String, #[source] std::io::Error),

    /// An error occurred when (de)serializing the credentials
    #[error("JSON error: {0}")]
    JSONError(#[from] serde_json::Error),
}

impl FileStorage {
    /// Create a new file storage with the given path
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Lock the file storage file for reading and writing. This will block until the lock is
    /// acquired.
    fn lock(&self) -> Result<LockFile, FileStorageError> {
        std::fs::create_dir_all(self.path.parent().unwrap())?;
        let path = self.path.with_extension("lock");
        let mut lock = fslock::LockFile::open(&path)
            .map_err(|e| FileStorageError::FailedToLock(path.to_string_lossy().into_owned(), e))?;

        // First try to lock the file without block. If we can't immediately get the lock we block and issue a debug message.
        if !lock
            .try_lock_with_pid()
            .map_err(|e| FileStorageError::FailedToLock(path.to_string_lossy().into_owned(), e))?
        {
            tracing::debug!("waiting for lock on {}", path.to_string_lossy());
            lock.lock_with_pid().map_err(|e| {
                FileStorageError::FailedToLock(path.to_string_lossy().into_owned(), e)
            })?;
        }

        Ok(lock)
    }

    /// Read the JSON file and deserialize it into a HashMap, or return an empty HashMap if the file
    /// does not exist
    fn read_json(&self) -> Result<HashMap<String, Authentication>, FileStorageError> {
        if !self.path.exists() {
            static WARN_GUARD: Lazy<Mutex<HashSet<PathBuf>>> =
                Lazy::new(|| Mutex::new(HashSet::new()));
            let mut guard = WARN_GUARD.lock().unwrap();
            if !guard.insert(self.path.clone()) {
                tracing::warn!(
                    "Can't find path for file storage on {}",
                    self.path.to_string_lossy()
                );
            }
            return Ok(HashMap::new());
        }
        let file = std::fs::File::open(&self.path)?;
        let reader = std::io::BufReader::new(file);
        let dict = serde_json::from_reader(reader)?;
        Ok(dict)
    }

    /// Serialize the given HashMap and write it to the JSON file
    fn write_json(&self, dict: &HashMap<String, Authentication>) -> Result<(), FileStorageError> {
        let file = std::fs::File::create(&self.path)?;
        let writer = std::io::BufWriter::new(file);
        serde_json::to_writer(writer, dict)?;
        Ok(())
    }
}

impl StorageBackend for FileStorage {
    fn store(&self, host: &str, authentication: &crate::Authentication) -> Result<()> {
        let _lock = self.lock()?;
        let mut dict = self.read_json()?;
        dict.insert(host.to_string(), authentication.clone());
        Ok(self.write_json(&dict)?)
    }

    fn get(&self, host: &str) -> Result<Option<crate::Authentication>> {
        let _lock = self.lock()?;
        let dict = self.read_json()?;
        Ok(dict.get(host).cloned())
    }

    fn delete(&self, host: &str) -> Result<()> {
        let _lock = self.lock()?;
        let mut dict = self.read_json()?;
        dict.remove(host);
        Ok(self.write_json(&dict)?)
    }
}

impl Default for FileStorage {
    fn default() -> Self {
        let mut path = dirs::home_dir().unwrap();
        path.push(".rattler");
        path.push("credentials.json");
        Self { path }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_file_storage() {
        let file = tempdir().unwrap();
        let path = file.path().join("test.json");

        let storage = FileStorage::new(path.clone());

        assert_eq!(storage.get("test").unwrap(), None);

        storage
            .store("test", &Authentication::CondaToken("password".to_string()))
            .unwrap();
        assert_eq!(
            storage.get("test").unwrap(),
            Some(Authentication::CondaToken("password".to_string()))
        );

        storage.delete("test").unwrap();
        assert_eq!(storage.get("test").unwrap(), None);

        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"invalid json").unwrap();
        assert!(storage.get("test").is_err());
    }
}
