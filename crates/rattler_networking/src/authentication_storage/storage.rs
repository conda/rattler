//! Storage and access of authentication information

use anyhow::{anyhow, Result};
use reqwest::IntoUrl;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use url::Url;

use super::{
    authentication::Authentication,
    backends::{file::FileStorage, keyring::KeyringAuthenticationStorage},
    StorageBackend,
};

#[derive(Debug, Clone)]
/// This struct implements storage and access of authentication
/// information backed by multiple storage backends
/// (e.g. keyring and file storage)
/// Credentials are stored and retrieved from the backends in the
/// order they are added to the storage
pub struct AuthenticationStorage {
    backends: Vec<Arc<dyn StorageBackend + Send + Sync>>,
    cache: Arc<Mutex<HashMap<String, Option<Authentication>>>>,
}

impl Default for AuthenticationStorage {
    fn default() -> Self {
        let mut storage = Self::new();

        storage.add_backend(Arc::from(KeyringAuthenticationStorage::default()));
        storage.add_backend(Arc::from(FileStorage::default()));

        storage
    }
}

impl AuthenticationStorage {
    /// Create a new authentication storage with no backends
    pub fn new() -> Self {
        Self {
            backends: vec![],
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
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
                tracing::warn!("Error storing credentials in backend: {}", e);
            } else {
                return Ok(());
            }
        }

        Err(anyhow!("All backends failed to store credentials"))
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
                    return Ok(None);
                }
                Err(e) => {
                    tracing::warn!("Error retrieving credentials from backend: {}", e);
                }
            }
        }

        Err(anyhow!("All backends failed to retrieve credentials"))
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
            return Ok((url, None))
        };

        match self.get(host) {
            Ok(None) => {}
            Err(_) => return Ok((url, None)),
            Ok(Some(credentials)) => return Ok((url, Some(credentials))),
        };

        // Check for credentials under e.g. `*.prefix.dev`
        let Some(mut domain) = url.domain() else {
            return Ok((url, None))
        };

        loop {
            let wildcard_host = format!("*.{}", domain);

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
                tracing::warn!("Error deleting credentials from backend: {}", e);
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
