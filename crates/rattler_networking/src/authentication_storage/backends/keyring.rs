//! Backend to store credentials in the operating system's keyring

use keyring::Entry;
use std::str::FromStr;

use crate::{
    Authentication,
    authentication_storage::{AuthenticationStorageError, StorageBackend},
};

const INDEX_ACCOUNT: &str = "__rattler_authentication_hosts";
const WELL_KNOWN_HOSTS: &[&str] = &[
    "prefix.dev",
    "*.prefix.dev",
    "repo.prefix.dev",
    "anaconda.org",
    "*.anaconda.org",
];

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

    fn index_entry(&self) -> Result<Entry, KeyringAuthenticationStorageError> {
        Entry::new(&self.store_key, INDEX_ACCOUNT).map_err(KeyringAuthenticationStorageError::from)
    }

    fn read_index(&self) -> Result<Vec<String>, KeyringAuthenticationStorageError> {
        let entry = self.index_entry()?;
        let password = match entry.get_password() {
            Ok(password) => password,
            Err(keyring::Error::NoEntry) => return Ok(Vec::new()),
            Err(err) => return Err(KeyringAuthenticationStorageError::from(err)),
        };

        serde_json::from_str(&password).map_err(KeyringAuthenticationStorageError::from)
    }

    fn write_index(&self, hosts: &[String]) -> Result<(), KeyringAuthenticationStorageError> {
        let password =
            serde_json::to_string(hosts).map_err(KeyringAuthenticationStorageError::from)?;
        self.index_entry()?
            .set_password(&password)
            .map_err(KeyringAuthenticationStorageError::from)
    }

    fn add_to_index(&self, host: &str) -> Result<(), KeyringAuthenticationStorageError> {
        let mut hosts = self.read_index()?;
        if !hosts.iter().any(|existing| existing == host) {
            hosts.push(host.to_string());
            hosts.sort();
            self.write_index(&hosts)?;
        }
        Ok(())
    }

    fn remove_from_index(&self, host: &str) -> Result<(), KeyringAuthenticationStorageError> {
        let mut hosts = self.read_index()?;
        let previous_len = hosts.len();
        hosts.retain(|existing| existing != host);
        if hosts.len() != previous_len {
            self.write_index(&hosts)?;
        }
        Ok(())
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

        if let Err(err) = self.add_to_index(host) {
            tracing::debug!("Error updating keyring credential index: {err}");
        }

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

    fn list(&self) -> Result<Vec<(String, Authentication)>, AuthenticationStorageError> {
        let mut entries = Vec::new();
        let mut hosts = self.read_index()?;
        hosts.extend(WELL_KNOWN_HOSTS.iter().map(|host| host.to_string()));
        hosts.sort();
        hosts.dedup();

        for host in hosts {
            if let Some(auth) = self.get(&host)? {
                entries.push((host, auth));
            }
        }
        Ok(entries)
    }

    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
        let entry =
            Entry::new(&self.store_key, host).map_err(KeyringAuthenticationStorageError::from)?;

        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(err) => return Err(KeyringAuthenticationStorageError::from(err).into()),
        }

        if let Err(err) = self.remove_from_index(host) {
            tracing::debug!("Error updating keyring credential index: {err}");
        }

        Ok(())
    }
}
