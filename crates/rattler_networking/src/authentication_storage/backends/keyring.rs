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
    // `\Q...\E` quotes the store_key as a literal so any future caller using a
    // custom (possibly regex-meaningful) store key still gets the expected
    // entries back.
    HashMap::from([("pattern".to_string(), format!(r"\.\Q{store_key}\E\z"))])
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

    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
        let entry = self.entry(host)?;

        match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => {}
            Err(err) => return Err(KeyringAuthenticationStorageError::from(err).into()),
        }

        Ok(())
    }
}
