//! Storage and access of authentication information

use anyhow::{Result, anyhow};
use reqwest::IntoUrl;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};
use url::Url;

use crate::authentication_storage::{AuthenticationStorageError, backends::file::FileStorage};

use super::{StorageBackend, authentication::Authentication};

#[cfg(feature = "netrc-rs")]
use super::backends::netrc::NetRcStorage;

#[cfg(feature = "keyring")]
use crate::authentication_storage::backends::keyring::KeyringAuthenticationStorageError;

#[cfg(feature = "keyring")]
use super::backends::keyring::KeyringAuthenticationStorage;

/// A single entry returned by [`AuthenticationStorage::list_keys_with_sources`].
/// Carries host metadata without the stored credential, so callers can build
/// UIs (pickers, status displays) without forcing a secret read from every
/// backend.
#[derive(Debug, Clone)]
pub struct LazyListedEntry {
    /// The host this credential is stored under.
    pub host: String,
    /// Human-readable name of the backend the entry came from.
    pub source: String,
    /// `true` if this is the entry [`get`](AuthenticationStorage::get) would
    /// return; later backends with the same host are "shadowed".
    pub active: bool,
}

/// A single entry returned by [`AuthenticationStorage::list_with_sources`].
/// Carries the host and credential along with the backend's display name and
/// a flag indicating whether this is the entry [`get`](AuthenticationStorage::get)
/// would actually return (the first backend that knows the host wins).
#[derive(Debug, Clone)]
pub struct ListedEntry {
    /// The host this credential is stored under.
    pub host: String,
    /// The credential itself.
    pub auth: Authentication,
    /// Human-readable name of the backend the entry came from (see
    /// [`StorageBackend::name`]).
    pub source: String,
    /// `true` if this is the entry `get(host)` would return — i.e. the first
    /// backend (in priority order) that holds credentials for `host`. Later
    /// backends with the same host are "shadowed" and have `active = false`.
    pub active: bool,
}

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
                if matches!(
                    error,
                    AuthenticationStorageError::KeyringStorageError(
                        KeyringAuthenticationStorageError::StorageError(_)
                            | KeyringAuthenticationStorageError::UnsupportedTarget { .. }
                    )
                ) {
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
                    if matches!(
                        _e,
                        AuthenticationStorageError::KeyringStorageError(
                            KeyringAuthenticationStorageError::StorageError(_)
                                | KeyringAuthenticationStorageError::UnsupportedTarget { .. }
                        )
                    ) {
                        tracing::trace!("Error storing credentials in keyring: {}", _e);
                    } else {
                        tracing::warn!("Error retrieving credentials from backend: {}", _e);
                    }
                }
            }
        }

        // Cache the negative result to avoid repeated backend lookups
        // (especially important for keyring which uses D-Bus IPC on Linux).
        let mut cache = self.cache.lock().unwrap();
        cache.insert(host.to_string(), None);

        Ok(None)
    }

    /// List authentication entries known to the configured backends.
    ///
    /// Entries are deduplicated by host using backend priority, matching the
    /// lookup behavior of [`get`](Self::get).
    pub fn list(&self) -> Result<Vec<(String, Authentication)>> {
        let mut entries: BTreeMap<String, Authentication> = BTreeMap::new();

        for backend in &self.backends {
            match backend.list() {
                Ok(backend_entries) => {
                    for (host, auth) in backend_entries {
                        entries.entry(host).or_insert(auth);
                    }
                }
                Err(error) => {
                    tracing::warn!("Error listing credentials from backend: {}", error);
                }
            }
        }

        Ok(entries.into_iter().collect())
    }

    /// Like [`list`](Self::list), but reports every entry from every backend
    /// (not deduplicated) along with the backend's human-readable name (see
    /// [`StorageBackend::name`]) and whether it's the entry that `get()` would
    /// return for that host. Used by `auth status` so users can see what's
    /// stored where, including shadowed entries.
    pub fn list_with_sources(&self) -> Result<Vec<ListedEntry>> {
        let mut entries: Vec<ListedEntry> = Vec::new();
        let mut seen_hosts: std::collections::HashSet<String> = std::collections::HashSet::new();

        for backend in &self.backends {
            match backend.list() {
                Ok(backend_entries) => {
                    let source = backend.name();
                    for (host, auth) in backend_entries {
                        let active = seen_hosts.insert(host.clone());
                        entries.push(ListedEntry {
                            host,
                            auth,
                            source: source.clone(),
                            active,
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!("Error listing credentials from backend: {}", error);
                }
            }
        }

        entries.sort_by(|a, b| a.host.cmp(&b.host));
        Ok(entries)
    }

    /// Retrieve the authentication information for the given URL, along with the
    /// storage key that matched (exact host or wildcard).
    ///
    /// This is useful when the caller needs to store updated credentials back
    /// under the same key (e.g. after an OAuth token refresh).
    ///
    /// Returns `(url, Some((matched_key, auth)))` or `(url, None)`.
    pub fn get_by_url_with_host<U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<(Url, Option<(String, Authentication)>), reqwest::Error> {
        let url = url.into_url()?;
        let host = match url.host_str() {
            Some(h) => h.to_string(),
            None => return Ok((url, None)),
        };

        match self.get(&host) {
            Ok(None) => {}
            Err(_) => return Ok((url, None)),
            Ok(Some(credentials)) => {
                return Ok((url, Some((host, credentials))));
            }
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
                            _ => return Ok((url, None)), // No more sub-paths to check
                        }
                    }
                    Ok(Some(credentials)) => {
                        return Ok((url, Some((current_url.as_str().to_string(), credentials))));
                    }
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
                return Ok((url, Some((wildcard_host, credentials))));
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
        let (url, auth) = self.get_by_url_with_host(url)?;
        Ok((url, auth.map(|(_, credentials)| credentials)))
    }

    /// Like [`get_by_url`](Self::get_by_url), but additionally refreshes
    /// expired OAuth access tokens via the provider's token endpoint
    /// before returning. Refreshed credentials are written back to the
    /// storage so subsequent calls see the new token.
    ///
    /// Non-OAuth credentials (bearer tokens, basic auth, S3, etc.) are
    /// returned unchanged.
    pub async fn get_by_url_refreshed<U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<(Url, Option<Authentication>), reqwest::Error> {
        let (url, auth_with_key) = self.get_by_url_with_host(url)?;
        let auth = match auth_with_key {
            // `maybe_refresh_oauth` is a no-op for non-OAuth variants and
            // returns them as-is, so this branch covers every auth type.
            Some((matched_key, auth)) => {
                crate::oauth_refresh::maybe_refresh_oauth(self, auth, &matched_key).await
            }
            None => None,
        };
        Ok((url, auth))
    }

    /// Lightweight variant of [`list_with_sources`](Self::list_with_sources)
    /// that returns only host metadata — never reading stored secrets.
    ///
    /// This lets callers build UIs (e.g. an interactive picker) without paying
    /// the per-entry keychain ACL prompts that `list_with_sources` would
    /// trigger on macOS. Callers that need the actual credential should call
    /// [`get_entry`](Self::get_entry) on the chosen `(host, source)` pair.
    pub fn list_keys_with_sources(&self) -> Result<Vec<LazyListedEntry>> {
        let mut entries: Vec<LazyListedEntry> = Vec::new();
        let mut seen_hosts: std::collections::HashSet<String> = std::collections::HashSet::new();

        for backend in &self.backends {
            match backend.list_keys() {
                Ok(hosts) => {
                    let source = backend.name();
                    for host in hosts {
                        let active = seen_hosts.insert(host.clone());
                        entries.push(LazyListedEntry {
                            host,
                            source: source.clone(),
                            active,
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!("Error listing credentials from backend: {}", error);
                }
            }
        }

        entries.sort_by(|a, b| a.host.cmp(&b.host));
        Ok(entries)
    }

    /// Fetch the credential for a specific `(host, source)` pair — i.e. read
    /// from the backend whose [`name`](StorageBackend::name) matches `source`.
    ///
    /// Used together with [`list_keys_with_sources`](Self::list_keys_with_sources)
    /// to defer secret reads until needed.
    pub fn get_entry(&self, host: &str, source: &str) -> Result<Option<Authentication>> {
        let backend = self
            .backends
            .iter()
            .find(|b| b.name() == source)
            .ok_or_else(|| {
                anyhow!(
                    "No configured backend named '{source}' is available to read the entry from"
                )
            })?;
        backend.get(host).map_err(Into::into)
    }

    /// Delete the entry stored under `host` in the backend identified by
    /// `source` (matching [`StorageBackend::name`]).
    ///
    /// Use this when callers want to surgically remove one backend's copy of a
    /// host without touching shadowed copies in other backends. For deleting
    /// every backend's copy of a host, see [`delete`](Self::delete).
    pub fn delete_entry(&self, host: &str, source: &str) -> Result<()> {
        // Drop the host from the cache entirely. Inserting `None` instead
        // would be read back by `get()` as a definitive "no credentials"
        // answer, hiding a shadowed copy in another backend; removing the
        // key forces `get()` to re-resolve against the backends.
        {
            let mut cache = self.cache.lock().unwrap();
            cache.remove(host);
        }

        let backend = self
            .backends
            .iter()
            .find(|b| b.name() == source)
            .ok_or_else(|| {
                anyhow!(
                    "No configured backend named '{source}' is available to delete the entry from"
                )
            })?;

        backend.delete(host).map_err(Into::into)
    }

    /// Delete the authentication information for the given host from every
    /// backend that holds it.
    pub fn delete(&self, host: &str) -> Result<()> {
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(host.to_string(), None);
        }

        let mut all_failed = true;

        for backend in &self.backends {
            if let Err(error) = backend.delete(host) {
                if is_benign_storage_error(&error) {
                    tracing::debug!("Backend ignored delete request: {}", error);
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

/// Errors that mean "this backend can't or doesn't need to do this" rather
/// than a real failure: read-only backends (netrc), platform keyrings that
/// don't support the requested operation, etc. These shouldn't WARN — they're
/// expected when multiple backends are layered.
fn is_benign_storage_error(error: &AuthenticationStorageError) -> bool {
    #[cfg(feature = "keyring")]
    if matches!(
        error,
        AuthenticationStorageError::KeyringStorageError(
            KeyringAuthenticationStorageError::StorageError(_)
                | KeyringAuthenticationStorageError::UnsupportedTarget { .. }
        )
    ) {
        return true;
    }
    #[cfg(feature = "netrc-rs")]
    if matches!(
        error,
        AuthenticationStorageError::NetRcStorageError(
            crate::authentication_storage::backends::netrc::NetRcStorageError::NotSupportedError(_)
        )
    ) {
        return true;
    }
    let _ = error;
    false
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::authentication_storage::backends::memory::MemoryStorage;

    fn storage_with(host: &str, auth: Authentication) -> AuthenticationStorage {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        storage.store(host, &auth).unwrap();
        storage
    }

    /// Non-OAuth credentials must pass through `get_by_url_refreshed`
    /// unchanged — the refresh path only applies to OAuth.
    #[tokio::test]
    async fn get_by_url_refreshed_passes_through_non_oauth() {
        let cases = [
            Authentication::BearerToken("bearer".into()),
            Authentication::CondaToken("conda".into()),
            Authentication::BasicHTTP {
                username: "u".into(),
                password: "p".into(),
            },
            Authentication::S3Credentials {
                access_key_id: "k".into(),
                secret_access_key: "s".into(),
                session_token: None,
            },
        ];

        for auth in cases {
            let storage = storage_with("example.com", auth.clone());
            let (_, retrieved) = storage
                .get_by_url_refreshed("https://example.com/foo")
                .await
                .unwrap();
            assert_eq!(retrieved, Some(auth));
        }
    }

    #[test]
    fn list_returns_entries_from_backends() {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(MemoryStorage::new()));
        storage
            .store(
                "example.com",
                &Authentication::BearerToken("token".to_string()),
            )
            .unwrap();

        assert_eq!(
            storage.list().unwrap(),
            vec![(
                "example.com".to_string(),
                Authentication::BearerToken("token".to_string())
            )]
        );
    }

    /// After surgically deleting one backend's copy via `delete_entry`,
    /// `get()` must re-resolve against the backends and return the shadowed
    /// copy from the next backend, not a stale cached result.
    #[test]
    fn delete_entry_unshadows_other_backends_for_get() {
        let mut storage = AuthenticationStorage::empty();
        let backend_a = Arc::new(MemoryStorage::with_name("a"));
        let backend_b = Arc::new(MemoryStorage::with_name("b"));
        storage.add_backend(backend_a.clone());
        storage.add_backend(backend_b.clone());

        backend_a
            .store("prefix.dev", &Authentication::BearerToken("tok-a".into()))
            .unwrap();
        backend_b
            .store("prefix.dev", &Authentication::BearerToken("tok-b".into()))
            .unwrap();

        // Prime the cache with the active (backend A) copy.
        assert_eq!(
            storage.get("prefix.dev").unwrap(),
            Some(Authentication::BearerToken("tok-a".into()))
        );

        storage
            .delete_entry("prefix.dev", &backend_a.name())
            .unwrap();

        assert_eq!(
            storage.get("prefix.dev").unwrap(),
            Some(Authentication::BearerToken("tok-b".into())),
            "shadowed copy must become visible after the active copy is deleted"
        );
    }

    #[test]
    fn entry_operations_reject_unknown_source() {
        let storage = storage_with("example.com", Authentication::BearerToken("t".into()));
        assert!(storage.get_entry("example.com", "no-such-backend").is_err());
        assert!(
            storage
                .delete_entry("example.com", "no-such-backend")
                .is_err()
        );
    }
}
