//! Backend to store credentials in the operating system's keyring

use keyring_core::{Entry, api::CredentialStore};
use std::{collections::HashMap, str::FromStr, sync::Arc};

use crate::{
    Authentication,
    authentication_storage::{AuthenticationStorageError, StorageBackend},
};

fn configure_default_store() -> Result<(), KeyringAuthenticationStorageError> {
    if keyring_core::get_default_store().is_some() {
        Ok(())
    } else {
        configure_platform_default_store()
    }
}

#[cfg(target_os = "macos")]
fn configure_platform_default_store() -> Result<(), KeyringAuthenticationStorageError> {
    keyring_core::set_default_store(apple_native_keyring_store::keychain::Store::new()?);
    Ok(())
}

#[cfg(target_os = "windows")]
fn configure_platform_default_store() -> Result<(), KeyringAuthenticationStorageError> {
    keyring_core::set_default_store(windows_native_keyring_store::Store::new()?);
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
fn configure_platform_default_store() -> Result<(), KeyringAuthenticationStorageError> {
    keyring_core::set_default_store(dbus_secret_service_keyring_store::Store::new()?);
    Ok(())
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(unix, not(any(target_os = "macos", target_os = "ios")))
)))]
fn configure_platform_default_store() -> Result<(), KeyringAuthenticationStorageError> {
    Err(KeyringAuthenticationStorageError::UnsupportedTarget {
        target: std::env::consts::OS.to_string(),
    })
}

/// Build the platform-specific [`CredentialStore::search`] spec that enumerates
/// every entry written by this storage instance.
///
/// macOS and the dbus secret service filter on the `service` attribute
/// directly. Windows has no notion of a "service" field — the keyring-core
/// store encodes `service` into the credential target as `{user}.{service}`
/// (default delimiters) and exposes a `pattern` (regex) filter, so we match on
/// the suffix.
#[cfg(any(
    target_os = "macos",
    all(unix, not(any(target_os = "macos", target_os = "ios")))
))]
fn search_spec(store_key: &str) -> HashMap<String, String> {
    HashMap::from([("service".to_string(), store_key.to_string())])
}

#[cfg(target_os = "windows")]
fn search_spec(store_key: &str) -> HashMap<String, String> {
    HashMap::from([("pattern".to_string(), windows_search_pattern(store_key))])
}

/// The regex handed to the Windows store's `pattern` search filter, matching
/// every credential target this storage writes (`{host}.{store_key}`).
///
/// The store compiles it with the Rust `regex` crate, so the store key must be
/// escaped with [`regex::escape`] — PCRE-style `\Q...\E` quoting is rejected as
/// an invalid pattern, which silently empties `list()`/`list_keys()` and broke
/// `auth logout` on Windows. Compiled (and tested) on every platform so a bad
/// pattern fails CI everywhere, not just on Windows.
#[cfg(any(target_os = "windows", test))]
fn windows_search_pattern(store_key: &str) -> String {
    format!(r"\.{}\z", regex::escape(store_key))
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(unix, not(any(target_os = "macos", target_os = "ios")))
)))]
fn search_spec(_store_key: &str) -> HashMap<String, String> {
    HashMap::new()
}

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

    fn entry(&self, host: &str) -> Result<Entry, KeyringAuthenticationStorageError> {
        configure_default_store()?;
        Entry::new(&self.store_key, host).map_err(KeyringAuthenticationStorageError::from)
    }
}

fn credential_store() -> Result<Arc<CredentialStore>, KeyringAuthenticationStorageError> {
    configure_default_store()?;
    keyring_core::get_default_store().ok_or_else(|| {
        KeyringAuthenticationStorageError::UnsupportedTarget {
            target: std::env::consts::OS.to_string(),
        }
    })
}

/// An error that can occur when accessing the authentication storage
#[derive(thiserror::Error, Debug)]
pub enum KeyringAuthenticationStorageError {
    // TODO: make this more fine-grained
    /// An error occurred when accessing the authentication storage
    #[error("Could not retrieve credentials from authentication storage: {0}")]
    StorageError(#[from] keyring_core::Error),

    /// The current target does not have a configured keyring-core store.
    #[error("No keyring-core credential store is configured for {target}")]
    UnsupportedTarget {
        /// Target OS without a configured keyring-core store.
        target: String,
    },

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
    fn name(&self) -> String {
        #[cfg(target_os = "macos")]
        {
            "macOS keychain".to_string()
        }
        #[cfg(target_os = "windows")]
        {
            "Windows credential manager".to_string()
        }
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
        {
            "secret service (keyring)".to_string()
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            all(unix, not(any(target_os = "macos", target_os = "ios")))
        )))]
        {
            "keyring".to_string()
        }
    }

    fn store(
        &self,
        host: &str,
        authentication: &Authentication,
    ) -> Result<(), AuthenticationStorageError> {
        let password = serde_json::to_string(authentication)
            .map_err(KeyringAuthenticationStorageError::from)?;
        let entry = self.entry(host)?;

        entry
            .set_password(&password)
            .map_err(KeyringAuthenticationStorageError::from)?;

        Ok(())
    }

    fn get(&self, host: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        let entry = self.entry(host)?;
        let password = entry.get_password();

        let p_string = match password {
            Ok(password) => password,
            Err(keyring_core::Error::NoEntry) => return Ok(None),
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
        let store = credential_store()?;
        let spec = search_spec(&self.store_key);
        let spec_refs: HashMap<&str, &str> =
            spec.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

        let entries = store
            .search(&spec_refs)
            .map_err(KeyringAuthenticationStorageError::from)?;

        let mut results = Vec::new();
        for entry in entries {
            let Some((service, account)) = entry.get_specifiers() else {
                continue;
            };
            // Defensive: on Windows the regex may match credentials whose
            // service component coincidentally ends in our store_key.
            if service != self.store_key {
                continue;
            }

            let password = match entry.get_password() {
                Ok(password) => password,
                Err(keyring_core::Error::NoEntry) => continue,
                Err(err) => return Err(KeyringAuthenticationStorageError::from(err).into()),
            };

            match Authentication::from_str(&password) {
                Ok(auth) => results.push((account, auth)),
                Err(err) => {
                    tracing::warn!("Error parsing credentials for {account}: {err:?}");
                }
            }
        }

        results.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(results)
    }

    /// Enumerate stored hosts without reading their passwords. On macOS this
    /// avoids one keychain ACL prompt per entry — important for callers like
    /// the `auth logout` interactive picker that only need host metadata to
    /// build their UI.
    fn list_keys(&self) -> Result<Vec<String>, AuthenticationStorageError> {
        let store = credential_store()?;
        let spec = search_spec(&self.store_key);
        let spec_refs: HashMap<&str, &str> =
            spec.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

        let entries = store
            .search(&spec_refs)
            .map_err(KeyringAuthenticationStorageError::from)?;

        let mut hosts = Vec::new();
        for entry in entries {
            let Some((service, account)) = entry.get_specifiers() else {
                continue;
            };
            if service != self.store_key {
                continue;
            }
            hosts.push(account);
        }
        hosts.sort();
        Ok(hosts)
    }

    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
        let entry = self.entry(host)?;

        match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => {}
            Err(err) => return Err(KeyringAuthenticationStorageError::from(err).into()),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyring_core::api::{CredentialApi, CredentialStoreApi};
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    /// Shared state of [`CountingStore`]: the stored secrets plus a counter
    /// of how often a secret was actually read.
    #[derive(Debug, Default)]
    struct StoreState {
        secrets: Mutex<HashMap<(String, String), Vec<u8>>>,
        secret_reads: AtomicUsize,
    }

    /// In-memory keyring-core store that counts secret reads. Used to assert
    /// that key enumeration never touches stored secrets — on macOS every
    /// secret read of a foreign-owned item triggers a keychain ACL prompt,
    /// so a regression here means one prompt per stored credential.
    #[derive(Debug, Default)]
    struct CountingStore {
        state: Arc<StoreState>,
    }

    #[derive(Debug)]
    struct CountingCred {
        state: Arc<StoreState>,
        service: String,
        account: String,
    }

    impl CountingCred {
        fn key(&self) -> (String, String) {
            (self.service.clone(), self.account.clone())
        }
    }

    impl CredentialApi for CountingCred {
        fn set_secret(&self, secret: &[u8]) -> keyring_core::Result<()> {
            self.state
                .secrets
                .lock()
                .unwrap()
                .insert(self.key(), secret.to_vec());
            Ok(())
        }

        fn get_secret(&self) -> keyring_core::Result<Vec<u8>> {
            self.state.secret_reads.fetch_add(1, Ordering::SeqCst);
            self.state
                .secrets
                .lock()
                .unwrap()
                .get(&self.key())
                .cloned()
                .ok_or(keyring_core::Error::NoEntry)
        }

        fn delete_credential(&self) -> keyring_core::Result<()> {
            self.state
                .secrets
                .lock()
                .unwrap()
                .remove(&self.key())
                .map(|_| ())
                .ok_or(keyring_core::Error::NoEntry)
        }

        fn get_credential(
            &self,
        ) -> keyring_core::Result<Option<Arc<keyring_core::api::Credential>>> {
            Ok(None)
        }

        fn get_specifiers(&self) -> Option<(String, String)> {
            Some((self.service.clone(), self.account.clone()))
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    impl CountingStore {
        fn entry(&self, service: &str, account: &str) -> Entry {
            Entry::new_with_credential(Arc::new(CountingCred {
                state: self.state.clone(),
                service: service.to_string(),
                account: account.to_string(),
            }))
        }
    }

    impl CredentialStoreApi for CountingStore {
        fn vendor(&self) -> String {
            "rattler-test".to_string()
        }

        fn id(&self) -> String {
            "counting-store".to_string()
        }

        fn build(
            &self,
            service: &str,
            user: &str,
            _modifiers: Option<&HashMap<&str, &str>>,
        ) -> keyring_core::Result<Entry> {
            Ok(self.entry(service, user))
        }

        fn search(&self, spec: &HashMap<&str, &str>) -> keyring_core::Result<Vec<Entry>> {
            // Honor the `service` filter (used on macOS/Linux); for any other
            // spec (e.g. Windows' `pattern`) return everything and rely on
            // the caller's defensive service check.
            let service_filter = spec.get("service").map(ToString::to_string);
            let entries = self
                .state
                .secrets
                .lock()
                .unwrap()
                .keys()
                .filter(|(service, _)| {
                    service_filter
                        .as_ref()
                        .is_none_or(|filter| service == filter)
                })
                .map(|(service, account)| self.entry(service, account))
                .collect();
            Ok(entries)
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// The Windows store compiles the search pattern with the Rust `regex`
    /// crate. An invalid pattern (like the `\Q...\E` quoting this used to
    /// emit) makes every `search()` fail, which empties `list()`/`list_keys()`
    /// and made `auth logout` report "No stored credentials found" for
    /// credentials that were right there in the Windows Credential Manager.
    #[test]
    fn windows_search_pattern_is_a_valid_regex_matching_target_names() {
        let pattern = windows_search_pattern("rattler");
        let re = regex::Regex::new(&pattern).expect("search pattern must compile");

        // Targets are written as `{host}.{store_key}`.
        assert!(re.is_match("*.example.org.rattler"));
        assert!(re.is_match("repo.prefix.dev.rattler"));
        // The store key must terminate the target...
        assert!(!re.is_match("*.example.org.rattler-build"));
        // ...and be preceded by a delimiter, not embedded in another word.
        assert!(!re.is_match("rattler.example.org"));

        // Regex metacharacters in a custom store key are matched literally.
        let pattern = windows_search_pattern("my+key");
        let re = regex::Regex::new(&pattern).expect("search pattern must compile");
        assert!(re.is_match("example.org.my+key"));
        assert!(!re.is_match("example.org.myyykey"));
    }

    /// The whole point of `list_keys` is to enumerate hosts without reading
    /// secrets (each read of a foreign-owned item prompts on macOS). Guard
    /// against a regression to the `list()` fallback, which reads every one.
    #[test]
    fn list_keys_does_not_read_secrets() {
        // The default store is process-global. This must stay the only test
        // in this binary that installs one: it keeps the real OS keyring out
        // of every test (configure_default_store never overrides an existing
        // store) and avoids races between concurrent installers.
        let store = Arc::new(CountingStore::default());
        keyring_core::set_default_store(store.clone());

        let backend = KeyringAuthenticationStorage::from_key("rattler-test-list-keys");
        backend
            .store(
                "a.example.com",
                &Authentication::BearerToken("token-a".into()),
            )
            .unwrap();
        backend
            .store(
                "b.example.com",
                &Authentication::BearerToken("token-b".into()),
            )
            .unwrap();

        let keys = backend.list_keys().unwrap();
        assert_eq!(
            keys,
            vec!["a.example.com".to_string(), "b.example.com".to_string()]
        );
        assert_eq!(
            store.state.secret_reads.load(Ordering::SeqCst),
            0,
            "listing keys must not read stored secrets"
        );

        // Sanity-check that the counter counts: a credential lookup reads once.
        let auth = backend.get("a.example.com").unwrap();
        assert_eq!(auth, Some(Authentication::BearerToken("token-a".into())));
        assert_eq!(store.state.secret_reads.load(Ordering::SeqCst), 1);
    }
}
