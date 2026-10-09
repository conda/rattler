//! file storage for passwords.
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use crate::{
    Authentication,
    authentication_storage::{AuthenticationStorageError, StorageBackend},
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
    #[cfg(feature = "dirs")]
    pub fn new() -> Result<Self, FileStorageError> {
        let home_dir = dirs::home_dir().ok_or_else(|| {
            FileStorageError::IOError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not determine the home directory. Please ensure the $HOME environment variable is set.",
            ))
        })?;

        let path = home_dir.join(".rattler").join("credentials.json");

        Self::from_path(path)
    }

    /// Read the latest file contents while the caller holds the cache write lock.
    fn read_json(&self) -> Result<BTreeMap<String, Authentication>, FileStorageError> {
        Ok(FileStorageCache::from_path(&self.path)?.content)
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
        let mut writer = BufWriter::new(&mut temp_file);
        serde_json::to_writer(&mut writer, dict).map_err(std::io::Error::from)?;
        // BufWriter's Drop ignores flush errors. Never replace the old file
        // until the buffered credential bytes have been written successfully.
        writer.flush()?;
        drop(writer);
        temp_file
            .persist(&self.path)
            .map_err(std::io::Error::from)?;

        Ok(())
    }
}

impl StorageBackend for FileStorage {
    fn name(&self) -> String {
        format!("file ({})", self.path.display())
    }

    fn store(
        &self,
        host: &str,
        authentication: &crate::Authentication,
    ) -> Result<(), AuthenticationStorageError> {
        // Hold one lock through read/modify/replace, not just the cache update.
        // Different resource contexts share this file and must not lose each other's grants.
        let mut cache = self.cache.write().unwrap();
        let mut dict = self.read_json()?;
        dict.insert(host.to_string(), authentication.clone());
        self.write_json(&dict)?;
        cache.content = dict;
        Ok(())
    }

    fn get(&self, host: &str) -> Result<Option<crate::Authentication>, AuthenticationStorageError> {
        let cache = self.cache.read().unwrap();
        Ok(cache.content.get(host).cloned())
    }

    fn get_uncached(
        &self,
        host: &str,
    ) -> Result<Option<crate::Authentication>, AuthenticationStorageError> {
        let mut cache = self.cache.write().unwrap();
        cache.content = self.read_json()?;
        Ok(cache.content.get(host).cloned())
    }

    fn list(&self) -> Result<Vec<(String, crate::Authentication)>, AuthenticationStorageError> {
        let cache = self.cache.read().unwrap();
        Ok(cache
            .content
            .iter()
            .map(|(host, auth)| (host.clone(), auth.clone()))
            .collect())
    }

    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
        let mut cache = self.cache.write().unwrap();
        let mut dict = self.read_json()?;
        if dict.remove(host).is_some() {
            self.write_json(&dict)?;
        }
        cache.content = dict;
        Ok(())
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

    #[test]
    fn concurrent_updates_and_deletion_preserve_other_entries() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("credentials.json");
        let file = FileStorage::from_path(path.clone()).unwrap();
        file.store("delete-me", &Authentication::BearerToken("fixture".into()))
            .unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(10));
        let jobs: Vec<_> = (0..9)
            .map(|i| {
                let file = file.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    if i == 8 {
                        file.delete("delete-me").unwrap();
                    } else {
                        file.store(
                            &format!("key-{i}"),
                            &Authentication::BearerToken("fixture".into()),
                        )
                        .unwrap();
                    }
                })
            })
            .collect();
        barrier.wait();
        for job in jobs {
            job.join().unwrap();
        }
        let reopened = FileStorage::from_path(path).unwrap();
        assert_eq!(reopened.list().unwrap().len(), 8);
        assert!(reopened.get("delete-me").unwrap().is_none());
        for i in 0..8 {
            assert!(reopened.get(&format!("key-{i}")).unwrap().is_some());
        }
    }
}
