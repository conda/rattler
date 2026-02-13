//! Storage and access of authentication information

use anyhow::{anyhow, Result};
use reqwest::IntoUrl;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use url::Url;

use crate::authentication_storage::{backends::file::FileStorage, AuthenticationStorageError};

use super::{authentication::Authentication, StorageBackend};

#[cfg(feature = "netrc-rs")]
use super::backends::netrc::NetRcStorage;

#[cfg(feature = "keyring")]
use crate::authentication_storage::backends::keyring::KeyringAuthenticationStorageError;

#[cfg(feature = "keyring")]
use super::backends::keyring::KeyringAuthenticationStorage;
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
        #[cfg(feature = "keyring")]
        storage.add_backend(Arc::from(KeyringAuthenticationStorage::default()));
        #[cfg(feature = "dirs")]
        storage.add_backend(Arc::from(FileStorage::new()?));
        #[cfg(feature = "netrc-rs")]
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
            #[allow(unused_variables)]
            if let Err(error) = backend.store(host, authentication) {
                #[cfg(feature = "keyring")]
                if let AuthenticationStorageError::KeyringStorageError(
                    KeyringAuthenticationStorageError::StorageError(_),
                ) = error
                {
                    tracing::debug!("Error storing credentials in keyring: {}", error);
                } else {
                    tracing::warn!("Error storing credentials from backend: {}", error);
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
                Ok(None) => {}
                Err(_e) => {
                    #[cfg(feature = "keyring")]
                    if let AuthenticationStorageError::KeyringStorageError(
                        KeyringAuthenticationStorageError::StorageError(_),
                    ) = _e
                    {
                        tracing::trace!("Error storing credentials in keyring: {}", _e);
                    } else {
                        tracing::warn!("Error retrieving credentials from backend: {}", _e);
                    }
                }
            }
        }

        Ok(None)
    }

    /// Try to find credentials by matching host + path prefix (longest match first).
    ///
    /// For a URL like `https://example.com/org/repo/file.json`, tries:
    /// 1. `example.com/org/repo/file.json`
    /// 2. `example.com/org/repo`
    /// 3. `example.com/org`
    /// 4. `example.com`
    fn get_by_path_prefix(&self, host: &str, path: &str) -> Result<Option<Authentication>> {
        // Normalize: remove trailing slashes
        let path = path.trim_end_matches('/');

        // Start with full host+path
        let mut current = if path.is_empty() {
            host.to_string()
        } else {
            format!("{host}{path}")
        };

        loop {
            if let Some(auth) = self.get(&current)? {
                return Ok(Some(auth));
            }

            // Try to strip the last path segment
            match current.rsplit_once('/') {
                Some((parent, _)) if !parent.is_empty() => {
                    current = parent.to_string();
                }
                _ => break, // No more segments to strip
            }
        }

        Ok(None)
    }

    /// Try to find credentials using wildcard domain expansion.
    ///
    /// For a URL like `https://repo.prefix.dev/path`, tries:
    /// 1. `*.repo.prefix.dev`
    /// 2. `*.prefix.dev`
    /// 3. `*.dev`
    ///
    /// Note: Wildcards only match against the host, not paths.
    fn get_by_wildcard_domain(&self, url: &Url) -> Result<Option<Authentication>> {
        let Some(mut domain) = url.domain() else {
            return Ok(None);
        };

        loop {
            let wildcard_host = format!("*.{domain}");

            if let Some(auth) = self.get(&wildcard_host)? {
                return Ok(Some(auth));
            }

            // Try parent domain
            match domain.split_once('.') {
                Some((_, rest)) if !rest.is_empty() => {
                    domain = rest;
                }
                _ => return Ok(None), // No more subdomains
            }
        }
    }

    /// Retrieve the authentication information for the given URL using
    /// longest-prefix path matching, falling back to wildcard domain matching.
    ///
    /// ## Matching Order
    ///
    /// For a request to `https://repo.prefix.dev/org/repo/file.json`:
    ///
    /// 1. `repo.prefix.dev/org/repo/file.json` (exact path)
    /// 2. `repo.prefix.dev/org/repo`
    /// 3. `repo.prefix.dev/org`
    /// 4. `repo.prefix.dev` (host only)
    /// 5. `*.repo.prefix.dev` (wildcard fallback)
    /// 6. `*.prefix.dev`
    /// 7. `*.dev`
    ///
    /// Note: Wildcards only match against the host, not paths.
    pub fn get_by_url<U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<(Url, Option<Authentication>), reqwest::Error> {
        let url = url.into_url()?;
        let Some(host) = url.host_str() else {
            return Ok((url, None));
        };

        // For S3 URLs, use the full URL string (s3://bucket/path)
        // For other URLs, use host + path
        let (lookup_host, lookup_path) = if url.scheme() == "s3" {
            (url.as_str().trim_end_matches('/'), "")
        } else {
            (host, url.path())
        };

        // 1. Try path prefix matching (longest to shortest)
        match self.get_by_path_prefix(lookup_host, lookup_path) {
            Ok(Some(auth)) => return Ok((url, Some(auth))),
            Ok(None) => {}
            Err(_) => return Ok((url, None)),
        }

        // 2. Try wildcard domain expansion (host-only, no paths)
        match self.get_by_wildcard_domain(&url) {
            Ok(Some(auth)) => return Ok((url, Some(auth))),
            Ok(None) => {}
            Err(_) => return Ok((url, None)),
        }

        Ok((url, None))
    }

    /// Delete the authentication information for the given host
    pub fn delete(&self, host: &str) -> Result<()> {
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(host.to_string(), None);
        }

        let mut all_failed = true;

        for backend in &self.backends {
            #[allow(unused_variables)]
            if let Err(error) = backend.delete(host) {
                #[cfg(feature = "keyring")]
                if let AuthenticationStorageError::KeyringStorageError(
                    KeyringAuthenticationStorageError::StorageError(_),
                ) = error
                {
                    tracing::debug!("Error deleting credentials in keyring: {}", error);
                } else {
                    tracing::warn!("Error deleting credentials from backend: {}", error);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authentication_storage::backends::memory::MemoryStorage;

    #[test]
    fn test_path_prefix_matching() {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::default()));

        // Store credential under host+path
        let auth = Authentication::BearerToken("token-for-org".to_string());
        storage.store("example.com/org/repo", &auth).unwrap();

        // Request to subpath should match
        let (_, result) = storage
            .get_by_url("https://example.com/org/repo/file.json")
            .unwrap();
        assert_eq!(result, Some(auth.clone()));

        // Request to exact path should match
        let (_, result) = storage.get_by_url("https://example.com/org/repo").unwrap();
        assert_eq!(result, Some(auth.clone()));

        // Request to different path should NOT match
        let (_, result) = storage
            .get_by_url("https://example.com/other/path")
            .unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_multiple_credentials_per_host() {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::default()));

        // Store different credentials for different paths on same host
        let auth_one = Authentication::BearerToken("token-one".to_string());
        let auth_two = Authentication::BearerToken("token-two".to_string());

        storage.store("example.com/one", &auth_one).unwrap();
        storage.store("example.com/two", &auth_two).unwrap();

        // Requests to /one should get auth_one
        let (_, result) = storage
            .get_by_url("https://example.com/one/packages/file.json")
            .unwrap();
        assert_eq!(result, Some(auth_one.clone()));

        // Requests to /two should get auth_two
        let (_, result) = storage
            .get_by_url("https://example.com/two/packages/file.json")
            .unwrap();
        assert_eq!(result, Some(auth_two.clone()));

        // Requests to unknown path should get nothing (no host-only fallback stored)
        let (_, result) = storage
            .get_by_url("https://example.com/three/file.json")
            .unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_host_only_fallback() {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::default()));

        // Store credential under host only (no path)
        let auth = Authentication::BearerToken("fallback-token".to_string());
        storage.store("example.com", &auth).unwrap();

        // Any path on that host should match
        let (_, result) = storage
            .get_by_url("https://example.com/any/path/here")
            .unwrap();
        assert_eq!(result, Some(auth.clone()));

        // Root path should also match
        let (_, result) = storage.get_by_url("https://example.com/").unwrap();
        assert_eq!(result, Some(auth.clone()));

        // No path should also match
        let (_, result) = storage.get_by_url("https://example.com").unwrap();
        assert_eq!(result, Some(auth));
    }

    #[test]
    fn test_longest_match_wins() {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::default()));

        // Store credentials at different path depths
        let auth_short = Authentication::BearerToken("short".to_string());
        let auth_long = Authentication::BearerToken("long".to_string());

        storage.store("example.com/org", &auth_short).unwrap();
        storage.store("example.com/org/repo", &auth_long).unwrap();

        // Request to /org/repo/file should match the longer path
        let (_, result) = storage
            .get_by_url("https://example.com/org/repo/file.json")
            .unwrap();
        assert_eq!(result, Some(auth_long.clone()));

        // Request to /org/other should match the shorter path
        let (_, result) = storage
            .get_by_url("https://example.com/org/other/file.json")
            .unwrap();
        assert_eq!(result, Some(auth_short));
    }

    #[test]
    fn test_wildcard_fallback_after_path() {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::default()));

        // Store wildcard credential
        let auth = Authentication::BearerToken("wildcard-token".to_string());
        storage.store("*.example.com", &auth).unwrap();

        // Request to subdomain with path should fall through to wildcard
        let (_, result) = storage
            .get_by_url("https://repo.example.com/org/file.json")
            .unwrap();
        assert_eq!(result, Some(auth));
    }

    #[test]
    fn test_s3_path_matching() {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::default()));

        // Store S3 credential with path
        let auth = Authentication::S3Credentials {
            access_key_id: "key".to_string(),
            secret_access_key: "secret".to_string(),
            session_token: None,
        };
        storage.store("s3://bucket/prefix", &auth).unwrap();

        // Request to subpath should match
        let (_, result) = storage
            .get_by_url("s3://bucket/prefix/path/to/file")
            .unwrap();
        assert_eq!(result, Some(auth.clone()));

        // Request to different prefix should not match
        let (_, result) = storage.get_by_url("s3://bucket/other/path").unwrap();
        assert_eq!(result, None);
    }
}
