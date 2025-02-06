//! Backend to store credentials in the operating system's keyring

use keyring::Entry;
use std::str::FromStr;

use crate::{
    authentication_storage::{AuthenticationStorageError, StorageBackend},
    Authentication,
};

#[derive(Clone, Debug)]
/// A storage backend that stores credentials in the operating system's keyring
pub struct KeyringAuthenticationStorage {
    /// The `store_key` needs to be unique per program as it is stored
    /// in a global dictionary in the operating system
    pub store_key: String,
}

impl KeyringAuthenticationStorage {
    /// Create a new authentication storage with the given store key
    pub fn from_key(store_key: &str) -> Self {
        Self {
            store_key: store_key.to_string(),
        }
    }
}

/// An error that can occur when accessing the authentication storage
#[derive(thiserror::Error, Debug)]
pub enum KeyringAuthenticationStorageError {
    // TODO: make this more fine-grained
    /// An error occurred when accessing the authentication storage
    #[error("Could not retrieve credentials from authentication storage: {0}")]
    StorageError(#[from] keyring::Error),

    /// An error occurred when serializing the credentials
    #[error("Could not serialize credentials {0}")]
    SerializeCredentialsError(#[from] serde_json::Error),

    /// An error occurred when parsing the credentials
    #[error("Could not parse credentials stored for {host}")]
    ParseCredentialsError {
        /// The host for which the credentials could not be parsed
        host: String,
    },
}

impl Default for KeyringAuthenticationStorage {
    fn default() -> Self {
        Self::from_key("rattler")
    }
}

impl StorageBackend for KeyringAuthenticationStorage {
    fn store(
        &self,
        host: &str,
        authentication: &Authentication,
    ) -> Result<(), AuthenticationStorageError> {
        let password = serde_json::to_string(authentication)
            .map_err(KeyringAuthenticationStorageError::from)?;
        let entry =
            Entry::new(&self.store_key, host).map_err(KeyringAuthenticationStorageError::from)?;

        entry
            .set_password(&password)
            .map_err(KeyringAuthenticationStorageError::from)?;

        Ok(())
    }

    fn get(&self, host: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        let entry =
            Entry::new(&self.store_key, host).map_err(KeyringAuthenticationStorageError::from)?;
        let password = entry.get_password();

        let p_string = match password {
            Ok(password) => password,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(e) => return Err(KeyringAuthenticationStorageError::from(e))?,
        };

        match Authentication::from_str(&p_string) {
            Ok(auth) => Ok(Some(auth)),
            Err(err) => {
                tracing::warn!("Error parsing credentials for {}: {:?}", host, err);
                Err(KeyringAuthenticationStorageError::ParseCredentialsError {
                    host: host.to_string(),
                }
                .into())
            }
        }
    }

    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
        let entry =
            Entry::new(&self.store_key, host).map_err(KeyringAuthenticationStorageError::from)?;
        entry
            .delete_credential()
            .map_err(KeyringAuthenticationStorageError::from)?;

        Ok(())
    }
}
