//! Fallback storage for passwords.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// A struct that implements storage and access of authentication
/// information backed by a on-disk JSON file
#[derive(Clone)]
pub struct FallbackStorage {
    /// The path to the JSON file
    pub path: PathBuf,

    /// A mutex to ensure that only one thread accesses the file at a time
    mutex: Arc<Mutex<()>>,
}

/// An error that can occur when accessing the fallback storage
#[derive(thiserror::Error, Debug)]
pub enum FallbackStorageError {
    /// An IO error occurred when accessing the fallback storage
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    /// An error occurred when (de)serializing the credentials
    #[error("JSON error: {0}")]
    JSONError(#[from] serde_json::Error),
}

impl FallbackStorage {
    /// Create a new fallback storage with the given path
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            mutex: Arc::new(Mutex::new(())),
        }
    }

    /// Store the given authentication information for the given host
    pub fn set_password(&self, host: &str, password: &str) -> Result<(), FallbackStorageError> {
        let _lock = self.mutex.lock().unwrap();
        let mut dict = self.read_json()?;
        dict.insert(host.to_string(), password.to_string());
        self.write_json(&dict)
    }

    /// Retrieve the authentication information for the given host
    pub fn get_password(&self, host: &str) -> Result<Option<String>, FallbackStorageError> {
        let _lock = self.mutex.lock().unwrap();
        let dict = self.read_json()?;
        Ok(dict.get(host).cloned())
    }

    /// Delete the authentication information for the given host
    pub fn delete_password(&self, host: &str) -> Result<(), FallbackStorageError> {
        let _lock = self.mutex.lock().unwrap();
        let mut dict = self.read_json()?;
        dict.remove(host);
        self.write_json(&dict)
    }

    /// Read the JSON file and deserialize it into a HashMap, or return an empty HashMap if the file
    /// does not exist
    fn read_json(&self) -> Result<std::collections::HashMap<String, String>, FallbackStorageError> {
        if !self.path.exists() {
            tracing::warn!(
                "Can't find path for fallback storage on {}",
                self.path.to_string_lossy()
            );
            return Ok(std::collections::HashMap::new());
        }
        let file = std::fs::File::open(&self.path)?;
        let reader = std::io::BufReader::new(file);
        let dict = serde_json::from_reader(reader)?;
        Ok(dict)
    }

    /// Serialize the given HashMap and write it to the JSON file
    fn write_json(
        &self,
        dict: &std::collections::HashMap<String, String>,
    ) -> Result<(), FallbackStorageError> {
        if !self.path.exists() {
            std::fs::create_dir_all(self.path.parent().unwrap())?;
        }
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
