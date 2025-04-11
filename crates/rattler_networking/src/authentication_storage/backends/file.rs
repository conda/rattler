//! file storage for passwords.
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io::BufWriter,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use crate::{
    authentication_storage::{AuthenticationStorageError, StorageBackend},
    Authentication,
};

#[derive(Clone, Debug)]
struct FileStorageCache {
    content: BTreeMap<String, Authentication>,
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
    #[error(transparent)]
    IOError(#[from] std::io::Error),

    /// An error occurred when (de)serializing the credentials
    #[error("failed to parse {0}: {1}")]
    JSONError(PathBuf, serde_json::Error),
}

impl FileStorageCache {
    pub fn from_path(path: &Path) -> Result<Self, FileStorageError> {
        match fs_err::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                content: BTreeMap::new(),
            }),
            Err(e) => Err(FileStorageError::IOError(e)),
            Ok(content) => {
                let content = serde_json::from_str(&content)
                    .map_err(|e| FileStorageError::JSONError(path.to_path_buf(), e))?;
                Ok(Self { content })
            }
        }
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

    /// Updates the cache by reading the JSON file and deserializing it into a
    /// `BTreeMap`, or return an empty `BTreeMap` if the file does not exist
    fn read_json(&self) -> Result<BTreeMap<String, Authentication>, FileStorageError> {
        let new_cache = FileStorageCache::from_path(&self.path)?;
        let mut cache = self.cache.write().unwrap();
        cache.content = new_cache.content;
        Ok(cache.content.clone())
    }

    /// Serialize the given `BTreeMap` and write it to the JSON file
    fn write_json(&self, dict: &BTreeMap<String, Authentication>) -> Result<(), FileStorageError> {
        let parent = self
            .path
            .parent()
            .ok_or(FileStorageError::IOError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Parent directory not found",
            )))?;
        std::fs::create_dir_all(parent)?;

        let prefix = self
            .path
            .file_stem()
            .unwrap_or_else(|| OsStr::new("credentials"));
        let extension = self
            .path
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or("json");

        // Write the contents to a temporary file and then atomically move it to the
        // final location.
        let mut temp_file = tempfile::Builder::new()
            .prefix(prefix)
            .suffix(&format!(".{extension}"))
            .tempfile_in(parent)?;
        serde_json::to_writer(BufWriter::new(&mut temp_file), dict)
            .map_err(std::io::Error::from)?;
        temp_file
            .persist(&self.path)
            .map_err(std::io::Error::from)?;

        // Store the new data in the cache
        let mut cache = self.cache.write().unwrap();
        cache.content = dict.clone();

        Ok(())
    }
}

impl StorageBackend for FileStorage {
    fn store(
        &self,
        host: &str,
        authentication: &crate::Authentication,
    ) -> Result<(), AuthenticationStorageError> {
        let mut dict = self.read_json()?;
        dict.insert(host.to_string(), authentication.clone());
        Ok(self.write_json(&dict)?)
    }

    fn get(&self, host: &str) -> Result<Option<crate::Authentication>, AuthenticationStorageError> {
        let cache = self.cache.read().unwrap();
        Ok(cache.content.get(host).cloned())
    }

    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
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
    use std::{fs, io::Write};

    use insta::assert_snapshot;
    use tempfile::tempdir;

    use super::*;

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
