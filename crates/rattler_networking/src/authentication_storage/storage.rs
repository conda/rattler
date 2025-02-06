//! Storage and access of authentication information

use anyhow::{anyhow, Result};
use reqwest::IntoUrl;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use url::Url;

use crate::authentication_storage::{
    backends::{file::FileStorage, keyring::KeyringAuthenticationStorageError},
    AuthenticationStorageError,
};

use super::{
    authentication::Authentication,
    backends::{keyring::KeyringAuthenticationStorage, netrc::NetRcStorage},
    StorageBackend,
};

#[derive(Debug, Clone)]
/// This struct implements storage and access of authentication
/// information backed by multiple storage backends
/// (e.g. keyring and file storage)
/// Credentials are stored and retrieved from the backends in the
/// order they are added to the storage
pub struct AuthenticationStorage {
    /// Authentication backends
    pub backends: Vec<Arc<dyn StorageBackend + Send + Sync>>,
    cache: Arc<Mutex<HashMap<String, Option<Authentication>>>>,
}

impl AuthenticationStorage {
    /// Create a new authentication storage with no backends
    pub fn empty() -> Self {
        Self {
            backends: vec![],
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a new authentication storage with the default backends
    /// Following order:
    /// - file storage from `RATTLER_AUTH_FILE` (if set)
    /// - keyring storage
    /// - file storage from the default location
    /// - netrc storage
    pub fn from_env_and_defaults() -> Result<Self, AuthenticationStorageError> {
        let mut storage = Self::empty();

        if let Ok(auth_file) = std::env::var("RATTLER_AUTH_FILE") {
            let path = std::path::Path::new(&auth_file);
            tracing::info!(
                "\"RATTLER_AUTH_FILE\" environment variable set, using file storage at {}",
                auth_file
            );
            storage.add_backend(Arc::from(FileStorage::from_path(path.into())?));
        }
        storage.add_backend(Arc::from(KeyringAuthenticationStorage::default()));
        storage.add_backend(Arc::from(FileStorage::new()?));
        storage.add_backend(Arc::from(NetRcStorage::from_env().unwrap_or_else(
            |(path, err)| {
                tracing::warn!("error reading netrc file from {}: {}", path.display(), err);
                NetRcStorage::default()
            },
        )));

        Ok(storage)
    }

    /// Add a new storage backend to the authentication storage
    /// (backends are tried in the order they are added)
    pub fn add_backend(&mut self, backend: Arc<dyn StorageBackend + Send + Sync>) {
        self.backends.push(backend);
    }

    /// Store the given authentication information for the given host
    pub fn store(&self, host: &str, authentication: &Authentication) -> Result<()> {
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(host.to_string(), Some(authentication.clone()));
        }

        for backend in &self.backends {
            if let Err(e) = backend.store(host, authentication) {
                if let AuthenticationStorageError::KeyringStorageError(
                    KeyringAuthenticationStorageError::StorageError(_),
                ) = e
                {
                    tracing::debug!("Error storing credentials in keyring: {}", e);
                } else {
                    tracing::warn!("Error storing credentials from backend: {}", e);
                }
            } else {
                return Ok(());
            }
        }

        Err(anyhow!(
            "All backends failed to store credentials. Checked the following backends: {:?}",
            self.backends
        ))
    }

    /// Retrieve the authentication information for the given host
    pub fn get(&self, host: &str) -> Result<Option<Authentication>> {
        {
            let cache = self.cache.lock().unwrap();
            if let Some(auth) = cache.get(host) {
                return Ok(auth.clone());
            }
        }

        for backend in &self.backends {
            match backend.get(host) {
                Ok(Some(auth)) => {
                    let mut cache = self.cache.lock().unwrap();
                    cache.insert(host.to_string(), Some(auth.clone()));
                    return Ok(Some(auth));
                }
                Ok(None) => {
                    continue;
                }
                Err(e) => {
                    if let AuthenticationStorageError::KeyringStorageError(
                        KeyringAuthenticationStorageError::StorageError(_),
                    ) = e
                    {
                        tracing::trace!("Error storing credentials in keyring: {}", e);
                    } else {
                        tracing::warn!("Error retrieving credentials from backend: {}", e);
                    }
                }
            }
        }

        Ok(None)
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
        let Some(host) = url.host_str() else {
            return Ok((url, None));
        };

        match self.get(host) {
            Ok(None) => {}
            Err(_) => return Ok((url, None)),
            Ok(Some(credentials)) => return Ok((url, Some(credentials))),
        };

        // S3 protocol URLs need to be treated separately since they follow a different schema
        if url.scheme() == "s3" {
            let mut current_url = url.clone();
            loop {
                match self.get(current_url.as_str()) {
                    Ok(None) => {
                        let possible_rest =
                            current_url.as_str().rsplit_once('/').map(|(rest, _)| rest);

                        match possible_rest {
                            Some(rest) => {
                                if let Ok(new_url) = Url::parse(rest) {
                                    current_url = new_url;
                                } else {
                                    return Ok((url, None));
                                }
                            }
                            _ => return Ok((url, None)), // No more subpaths to check
                        }
                    }
                    Ok(Some(credentials)) => return Ok((url, Some(credentials))),
                    Err(_) => return Ok((url, None)),
                }
            }
        }

        // Check for credentials under e.g. `*.prefix.dev`
        let Some(mut domain) = url.domain() else {
            return Ok((url, None));
        };

        loop {
            let wildcard_host = format!("*.{domain}");

            let Ok(credentials) = self.get(&wildcard_host) else {
                return Ok((url, None));
            };

            if let Some(credentials) = credentials {
                return Ok((url, Some(credentials)));
            }

            let possible_rest = domain.split_once('.').map(|(_, rest)| rest);

            match possible_rest {
                Some(rest) => {
                    domain = rest;
                }
                _ => return Ok((url, None)), // No more subdomains to check
            }
        }
    }

    /// Delete the authentication information for the given host
    pub fn delete(&self, host: &str) -> Result<()> {
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(host.to_string(), None);
        }

        let mut all_failed = true;

        for backend in &self.backends {
            if let Err(e) = backend.delete(host) {
                if let AuthenticationStorageError::KeyringStorageError(
                    KeyringAuthenticationStorageError::StorageError(_),
                ) = e
                {
                    tracing::debug!("Error deleting credentials in keyring: {}", e);
                } else {
                    tracing::warn!("Error deleting credentials from backend: {}", e);
                }
            } else {
                all_failed = false;
            }
        }

        if all_failed {
            Err(anyhow!("All backends failed to delete credentials"))
        } else {
            Ok(())
        }
    }
}
