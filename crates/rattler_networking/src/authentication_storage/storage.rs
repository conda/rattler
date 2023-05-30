//! Storage and access of authentication information
use std::{
    path::{Path, PathBuf},
    str::FromStr,
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

    /// Fallback JSON location
    pub fallback_json_location: PathBuf,
}

impl AuthenticationStorage {
    /// Create a new authentication storage with the given store key
    pub fn new(store_key: &str, fallback_folder: &Path) -> AuthenticationStorage {
        let fallback_location = fallback_folder.join(format!("{}_auth_store.json", store_key));
        AuthenticationStorage {
            store_key: store_key.to_string(),
            fallback_json_location: fallback_location,
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
    /// TODO: This should be a separate error type
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
                    self.fallback_json_location.display()
                );
                let fallback_storage =
                    fallback_storage::FallbackStorage::new(self.fallback_json_location.clone());
                fallback_storage.set_password(host, &password)?;
            }
        }
        Ok(())
    }

    /// Retrieve the authentication information for the given host
    pub fn get(&self, host: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        let entry = Entry::new(&self.store_key, host)?;
        let password = entry.get_password();

        let p_string = match password {
            Ok(password) => password,
            Err(keyring::Error::NoEntry) => {
                return Ok(None);
            }
            Err(e) => {
                tracing::warn!(
                    "Error retrieving credentials for {}: {}, using fallback storage at {}",
                    host,
                    e,
                    self.fallback_json_location.display()
                );
                let fallback_storage =
                    fallback_storage::FallbackStorage::new(self.fallback_json_location.clone());
                let fb_pw = fallback_storage.get_password(host)?;
                if fb_pw.is_none() {
                    return Ok(None);
                }
                fb_pw.unwrap()
            }
        };

        Ok(Some(Authentication::from_str(&p_string).map_err(|_| {
            AuthenticationStorageError::ParseCredentialsError {
                host: host.to_string(),
            }
        })?))
    }

    /// Delete the authentication information for the given host
    pub fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
        let entry = Entry::new(&self.store_key, host)?;
        let _ = entry.delete_password().map_err(|e| {
            tracing::warn!(
                "Error deleting credentials for {}: {}, using fallback storage at {}",
                host,
                e,
                self.fallback_json_location.display()
            );
        });

        let fallback_storage =
            fallback_storage::FallbackStorage::new(self.fallback_json_location.clone());
        Ok(fallback_storage.delete_password(host)?)
    }

    /// Retrieve the authentication information for the given URL
    pub fn get_by_url<U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<(Url, Option<Authentication>), reqwest::Error> {
        let url = url.into_url()?;

        if let Some(host) = url.host_str() {
            let credentials = self.get(host);
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
