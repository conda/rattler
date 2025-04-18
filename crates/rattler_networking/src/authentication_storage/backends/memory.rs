//! in-memory storage for authentication information
use std::{collections::HashMap, sync::Mutex};

use crate::{
    authentication_storage::{AuthenticationStorageError, StorageBackend},
    Authentication,
};

/// A struct that implements storage and access of authentication
/// information backed by a in-memory hashmap
#[derive(Debug)]
pub struct MemoryStorage {
    store: Mutex<HashMap<String, Authentication>>,
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStorage {
    /// Create a new empty memory storage
    pub fn new() -> Self {
        Self {
            store: Default::default(),
        }
    }
}

/// An error that can occur when accessing the authentication storage
#[derive(thiserror::Error, Debug)]
pub enum MemoryStorageError {
    /// Could not lock the storage
    #[error("Could not lock the storage")]
    LockError,
}

impl StorageBackend for MemoryStorage {
    fn store(
        &self,
        host: &str,
        authentication: &Authentication,
    ) -> Result<(), AuthenticationStorageError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| MemoryStorageError::LockError)?;
        store.insert(host.to_string(), authentication.clone());
        Ok(())
    }

    fn get(&self, host: &str) -> Result<Option<crate::Authentication>, AuthenticationStorageError> {
        let store = self
            .store
            .lock()
            .map_err(|_| MemoryStorageError::LockError)?;
        Ok(store.get(host).cloned())
    }

    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| MemoryStorageError::LockError)?;
        store.remove(host);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_storage() {
        let storage = MemoryStorage::new();

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

        storage.delete("test").unwrap();
        assert_eq!(storage.get("test").unwrap(), None);
    }
}
