//! Backend to store credentials in the operating system's keyring

use anyhow::Result;
use keyring::Entry;
use std::str::FromStr;

use crate::{authentication_storage::StorageBackend, Authentication};

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
    fn store(&self, host: &str, authentication: &Authentication) -> Result<()> {
        let password = serde_json::to_string(authentication)?;
        let entry = Entry::new(&self.store_key, host)?;

        entry.set_password(&password)?;

        Ok(())
    }

    fn get(&self, host: &str) -> Result<Option<Authentication>> {
        let entry = Entry::new(&self.store_key, host)?;
        let password = entry.get_password();

        let p_string = match password {
            Ok(password) => password,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(e) => return Err(e.into()),
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

    fn delete(&self, host: &str) -> Result<()> {
        let entry = Entry::new(&self.store_key, host)?;
        entry.delete_credential()?;

        Ok(())
    }
}
