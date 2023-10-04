//! Storage and access of authentication information
use std::{
    collections::HashMap,
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
};

use keyring::Entry;
use reqwest::{IntoUrl, Url};

use super::{authentication::Authentication, fallback_storage};

/// A struct that implements storage and access of authentication
/// information
#[derive(Clone)]
pub struct AuthenticationStorage {
    /// The store_key needs to be unique per program as it is stored
    /// in a global dictionary in the operating system
    pub store_key: String,

    /// Fallback Storage that will be used if the is no key store application available.
    pub fallback_storage: fallback_storage::FallbackStorage,

    /// A cache so that we don't have to access the keyring all the time
    cache: Arc<Mutex<HashMap<String, Option<Authentication>>>>,
}

impl AuthenticationStorage {
    /// Create a new authentication storage with the given store key
    pub fn new(store_key: &str, fallback_folder: &Path) -> AuthenticationStorage {
        let fallback_location = fallback_folder.join(format!("{}_auth_store.json", store_key));
        AuthenticationStorage {
            store_key: store_key.to_string(),
            fallback_storage: fallback_storage::FallbackStorage::new(fallback_location),
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// An error that can occur when accessing the authentication storage
#[derive(thiserror::Error, Debug)]
pub enum AuthenticationStorageError {
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

    /// An error occurred when accessing the fallback storage
    /// (e.g. the JSON file)
    #[error("Could not retrieve credentials from fallback storage: {0}")]
    FallbackStorageError(#[from] fallback_storage::FallbackStorageError),
}

impl AuthenticationStorage {
    /// Store the given authentication information for the given host
    pub fn store(
        &self,
        host: &str,
        authentication: &Authentication,
    ) -> Result<(), AuthenticationStorageError> {
        let entry = Entry::new(&self.store_key, host)?;
        let password = serde_json::to_string(authentication)?;

        match entry.set_password(&password) {
            Ok(_) => return Ok(()),
            Err(e) => {
                tracing::warn!(
                    "Error storing credentials for {}: {}, using fallback storage at {}",
                    host,
                    e,
                    self.fallback_storage.path.display()
                );
                self.fallback_storage.set_password(host, &password)?;
            }
        }
        Ok(())
    }

    /// Retrieve the authentication information for the given host
    pub fn get(&self, host: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        {
            let cache = self.cache.lock().unwrap();
            if let Some(auth) = cache.get(host) {
                return Ok(auth.clone());
            }
        }

        let entry = Entry::new(&self.store_key, host)?;
        let password = entry.get_password();

        let p_string = match password {
            Ok(password) => password,
            Err(keyring::Error::NoEntry) => {
                return Ok(None);
            }
            Err(e) => {
                tracing::debug!(
                    "Unable to retrieve credentials for {}: {}, using fallback credential storage at {}",
                    host,
                    e,
                    self.fallback_storage.path.display()
                );
                match self.fallback_storage.get_password(host)? {
                    None => return Ok(None),
                    Some(password) => password,
                }
            }
        };

        match Authentication::from_str(&p_string) {
            Ok(auth) => {
                let mut cache = self.cache.lock().unwrap();
                cache.insert(host.to_string(), Some(auth.clone()));
                Ok(Some(auth))
            }
            Err(err) => {
                tracing::warn!("Error parsing credentials for {}: {:?}", host, err);
                Err(AuthenticationStorageError::ParseCredentialsError {
                    host: host.to_string(),
                })
            }
        }
    }

    /// Delete the authentication information for the given host
    pub fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
        {
            let mut cache = self.cache.lock().unwrap();
            cache.remove(host);
        }

        let entry = Entry::new(&self.store_key, host)?;
        match entry.delete_password() {
            Ok(_) => {}
            Err(keyring::Error::NoEntry) => {}
            Err(e) => {
                tracing::warn!("Error deleting credentials for {}: {}", host, e);
            }
        }

        Ok(self.fallback_storage.delete_password(host)?)
    }

    /// Retrieve the authentication information for the given URL
    /// (including the authentication information for the wildcard
    /// host if no credentials are found for the given host)
    ///
    /// E.g. if credentials are stored for `*.prefix.dev` and the
    /// given URL is `https://repo.prefix.dev`, the credentials
    /// for `*.prefix.dev` will be returned.
    pub fn get_by_url<U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<(Url, Option<Authentication>), reqwest::Error> {
        let url = url.into_url()?;
        if let Some(host) = url.host_str() {
            let credentials = self.get(host);

            let credentials = match credentials {
                Ok(None) => {
                    // Check for credentials under e.g. `*.prefix.dev`
                    let mut parts = host.rsplitn(2, '.').collect::<Vec<&str>>();
                    parts.reverse();
                    let wildcard_host = format!("*.{}", parts.join("."));
                    self.get(&wildcard_host)
                }
                _ => credentials,
            };

            match credentials {
                Ok(None) => Ok((url, None)),
                Ok(Some(credentials)) => Ok((url, Some(credentials))),
                Err(e) => {
                    tracing::warn!("Error retrieving credentials for {}: {}", host, e);
                    Ok((url, None))
                }
            }
        } else {
            Ok((url, None))
        }
    }
}
