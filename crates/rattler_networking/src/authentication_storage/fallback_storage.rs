//! Fallback storage for passwords.
use fslock::LockFile;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::{path::PathBuf, sync::Mutex};

/// A struct that implements storage and access of authentication
/// information backed by a on-disk JSON file
#[derive(Clone)]
pub struct FallbackStorage {
    /// The path to the JSON file
    pub path: PathBuf,
}

/// An error that can occur when accessing the fallback storage
#[derive(thiserror::Error, Debug)]
pub enum FallbackStorageError {
    /// An IO error occurred when accessing the fallback storage
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    /// Failed to lock the fallback storage file
    #[error("failed to lock fallback storage file {0}.")]
    FailedToLock(String, #[source] std::io::Error),

    /// An error occurred when (de)serializing the credentials
    #[error("JSON error: {0}")]
    JSONError(#[from] serde_json::Error),
}

impl FallbackStorage {
    /// Create a new fallback storage with the given path
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Lock the fallback storage file for reading and writing. This will block until the lock is
    /// acquired.
    fn lock(&self) -> Result<LockFile, FallbackStorageError> {
        std::fs::create_dir_all(self.path.parent().unwrap())?;
        let path = self.path.with_extension("lock");
        let mut lock = fslock::LockFile::open(&path).map_err(|e| {
            FallbackStorageError::FailedToLock(path.to_string_lossy().into_owned(), e)
        })?;

        // First try to lock the file without block. If we can't immediately get the lock we block and issue a debug message.
        if !lock.try_lock_with_pid().map_err(|e| {
            FallbackStorageError::FailedToLock(path.to_string_lossy().into_owned(), e)
        })? {
            tracing::debug!("waiting for lock on {}", path.to_string_lossy());
            lock.lock_with_pid().map_err(|e| {
                FallbackStorageError::FailedToLock(path.to_string_lossy().into_owned(), e)
            })?;
        }

        Ok(lock)
    }

    /// Store the given authentication information for the given host
    pub fn set_password(&self, host: &str, password: &str) -> Result<(), FallbackStorageError> {
        let _lock = self.lock()?;
        let mut dict = self.read_json()?;
        dict.insert(host.to_string(), password.to_string());
        self.write_json(&dict)
    }

    /// Retrieve the authentication information for the given host
    pub fn get_password(&self, host: &str) -> Result<Option<String>, FallbackStorageError> {
        let _lock = self.lock()?;
        let dict = self.read_json()?;
        Ok(dict.get(host).cloned())
    }

    /// Delete the authentication information for the given host
    pub fn delete_password(&self, host: &str) -> Result<(), FallbackStorageError> {
        let _lock = self.lock()?;
        let mut dict = self.read_json()?;
        dict.remove(host);
        self.write_json(&dict)
    }

    /// Read the JSON file and deserialize it into a `HashMap`, or return an empty `HashMap` if the file
    /// does not exist
    fn read_json(&self) -> Result<HashMap<String, String>, FallbackStorageError> {
        if !self.path.exists() {
            static WARN_GUARD: Lazy<Mutex<HashSet<PathBuf>>> =
                Lazy::new(|| Mutex::new(HashSet::new()));
            let mut guard = WARN_GUARD.lock().unwrap();
            if !guard.insert(self.path.clone()) {
                tracing::warn!(
                    "Can't find path for fallback storage on {}",
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

    /// Serialize the given `HashMap` and write it to the JSON file
    fn write_json(&self, dict: &HashMap<String, String>) -> Result<(), FallbackStorageError> {
        let file = std::fs::File::create(&self.path)?;
        let writer = std::io::BufWriter::new(file);
        serde_json::to_writer(writer, dict)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_fallback_storage() {
        let file = tempdir().unwrap();
        let path = file.path().join("test.json");

        let storage = FallbackStorage::new(path.clone());

        assert_eq!(storage.get_password("test").unwrap(), None);

        storage.set_password("test", "password").unwrap();
        assert_eq!(
            storage.get_password("test").unwrap(),
            Some("password".to_string())
        );

        storage.delete_password("test").unwrap();
        assert_eq!(storage.get_password("test").unwrap(), None);

        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"invalid json").unwrap();
        assert!(storage.get_password("test").is_err());
    }
}
