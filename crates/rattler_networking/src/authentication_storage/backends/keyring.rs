//! Backend to store credentials in the operating system's keyring

use keyring_core::{Entry, api::CredentialStore};
use std::{
    collections::HashMap,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use crate::{
    Authentication,
    authentication_storage::{AuthenticationStorageError, StorageBackend},
};

/// Default upper bound for a single keyring operation.
///
/// On Linux the keyring is the Secret Service, reached over D-Bus. When the
/// D-Bus session bus is unresponsive — e.g. `DBUS_SESSION_BUS_ADDRESS` points
/// at a `dbus-daemon` that has died, a common situation in headless `ssh -X`
/// sessions — the D-Bus handshake blocks forever with no timeout of its own,
/// which hangs the whole process (see prefix-dev/pixi#5682). Bounding every
/// keyring call lets rattler fall back to its other credential backends instead
/// of hanging indefinitely.
const DEFAULT_KEYRING_TIMEOUT: Duration = Duration::from_secs(10);

/// Latched to `true` once a keyring operation has timed out. Subsequent keyring
/// access is skipped for the remainder of the process: a single
/// [`get_by_url`](crate::AuthenticationStorage::get_by_url) performs several
/// host lookups (wildcard expansion) and a solve touches many hosts, so without
/// this latch a broken D-Bus would cost one full timeout per lookup.
static KEYRING_TIMED_OUT: AtomicBool = AtomicBool::new(false);

/// Whether `value` should be treated as "enabled/true". Recognizes `1`,
/// `true`, and `yes` (case-insensitive, surrounding whitespace ignored).
fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

/// Whether the user explicitly disabled the keyring backend via
/// `RATTLER_DISABLE_KEYRING`. This is an escape hatch for environments where the
/// OS keyring should never be consulted (e.g. CI, or an unauthenticated setup
/// where reaching for the keyring is pure overhead).
fn keyring_disabled_by_env() -> bool {
    std::env::var("RATTLER_DISABLE_KEYRING")
        .map(|value| is_truthy(&value))
        .unwrap_or(false)
}

/// Returns `false` when the keyring backend must be skipped entirely: either the
/// user disabled it via `RATTLER_DISABLE_KEYRING`, or a previous operation timed
/// out and latched it off for the rest of the process.
fn keyring_enabled() -> bool {
    !KEYRING_TIMED_OUT.load(Ordering::Relaxed) && !keyring_disabled_by_env()
}

/// The timeout to apply to a keyring operation, or `None` to run it inline with
/// no timeout.
///
/// The timeout only applies on the Linux Secret Service (D-Bus) backend, which
/// is the one that can hang indefinitely. On macOS and Windows the native
/// keyrings surface interactive unlock prompts that are expected to take as long
/// as the user needs, so wrapping them in a timeout would risk discarding a
/// perfectly good credential; there we always run inline.
///
/// `RATTLER_KEYRING_TIMEOUT` overrides the default (in whole seconds); a value
/// of `0` disables the timeout and restores the original blocking behavior.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
fn keyring_timeout() -> Option<Duration> {
    timeout_from_env_value(std::env::var("RATTLER_KEYRING_TIMEOUT").ok().as_deref())
}

/// Pure interpretation of the `RATTLER_KEYRING_TIMEOUT` value: unset falls back
/// to [`DEFAULT_KEYRING_TIMEOUT`], `0` disables the timeout (`None`), a valid
/// number is used verbatim, and anything else warns and falls back to the
/// default. Kept free of `std::env` access so it can be unit-tested without
/// mutating the process environment.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
fn timeout_from_env_value(value: Option<&str>) -> Option<Duration> {
    match value {
        None => Some(DEFAULT_KEYRING_TIMEOUT),
        Some(value) => match value.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(Duration::from_secs(secs)),
            Err(_) => {
                tracing::warn!(
                    "ignoring invalid RATTLER_KEYRING_TIMEOUT value {value:?}; \
                     expected a whole number of seconds, using the default of {}s",
                    DEFAULT_KEYRING_TIMEOUT.as_secs()
                );
                Some(DEFAULT_KEYRING_TIMEOUT)
            }
        },
    }
}

#[cfg(not(all(unix, not(any(target_os = "macos", target_os = "ios")))))]
fn keyring_timeout() -> Option<Duration> {
    None
}

/// Run a keyring operation with a timeout, latching the keyring off if it does
/// not complete in time. See [`run_with_timeout_inner`] for the details; this is
/// the production entry point that wires in the process-global latch.
fn run_with_timeout<T, F>(
    operation: &'static str,
    f: F,
) -> Result<T, KeyringAuthenticationStorageError>
where
    F: FnOnce() -> Result<T, KeyringAuthenticationStorageError> + Send + 'static,
    T: Send + 'static,
{
    run_with_timeout_inner(keyring_timeout(), &KEYRING_TIMED_OUT, operation, f)
}

/// Run `f`, returning [`KeyringAuthenticationStorageError::Timeout`] and setting
/// `latch` if it does not finish within `timeout`.
///
/// The work runs on a detached worker thread. If the underlying D-Bus call is
/// wedged on an unresponsive socket the worker may never return, but a detached
/// thread does not keep the process alive (it is torn down when `main` exits),
/// so leaking it is preferable to hanging. `timeout == None` runs `f` inline,
/// preserving the original blocking behavior.
fn run_with_timeout_inner<T, F>(
    timeout: Option<Duration>,
    latch: &AtomicBool,
    operation: &'static str,
    f: F,
) -> Result<T, KeyringAuthenticationStorageError>
where
    F: FnOnce() -> Result<T, KeyringAuthenticationStorageError> + Send + 'static,
    T: Send + 'static,
{
    let Some(timeout) = timeout else {
        return f();
    };

    let (tx, rx) = mpsc::channel();
    if let Err(err) = thread::Builder::new()
        .name("rattler-keyring".to_string())
        .spawn(move || {
            // The receiver may already be gone if we timed out; ignore that.
            let _ = tx.send(f());
        })
    {
        // Spawning the worker failed (extremely rare — e.g. the process is out
        // of threads). Treat the keyring as unavailable and fall back to the
        // other credential backends rather than failing outright.
        tracing::debug!("could not spawn keyring worker thread ({err}); skipping keyring");
        return Err(KeyringAuthenticationStorageError::Timeout { operation });
    }

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            latch.store(true, Ordering::Relaxed);
            tracing::warn!(
                "keyring '{operation}' did not respond within {}s; treating the OS keyring \
                 as unavailable for the rest of this run and falling back to other credential \
                 sources. This usually means the D-Bus session bus is unresponsive. Adjust or \
                 disable this timeout with RATTLER_KEYRING_TIMEOUT (seconds, 0 to disable), or \
                 skip the keyring entirely with RATTLER_DISABLE_KEYRING=1.",
                timeout.as_secs()
            );
            Err(KeyringAuthenticationStorageError::Timeout { operation })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // The worker panicked before sending a result.
            Err(KeyringAuthenticationStorageError::Timeout { operation })
        }
    }
}

/// Open the keyring entry for `host` under `store_key`, configuring the platform
/// default store if necessary. Free function (rather than a method) so it can be
/// moved into the worker closure of [`run_with_timeout`].
fn open_entry(store_key: &str, host: &str) -> Result<Entry, KeyringAuthenticationStorageError> {
    configure_default_store()?;
    Entry::new(store_key, host).map_err(KeyringAuthenticationStorageError::from)
}

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

    /// A keyring operation did not complete within the configured timeout
    /// (see [`DEFAULT_KEYRING_TIMEOUT`] and `RATTLER_KEYRING_TIMEOUT`). Usually
    /// caused by an unresponsive D-Bus session bus on Linux.
    #[error("Keyring operation '{operation}' timed out")]
    Timeout {
        /// The keyring operation that timed out (e.g. `get`, `store`).
        operation: &'static str,
    },

    /// The keyring backend was skipped because it is disabled — either via
    /// `RATTLER_DISABLE_KEYRING`, or latched off after an earlier timeout.
    #[error("Keyring backend is disabled")]
    Disabled,

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
        if !keyring_enabled() {
            return Err(KeyringAuthenticationStorageError::Disabled.into());
        }

        let password = serde_json::to_string(authentication)
            .map_err(KeyringAuthenticationStorageError::from)?;
        let store_key = self.store_key.clone();
        let host = host.to_string();

        run_with_timeout("store", move || {
            let entry = open_entry(&store_key, &host)?;
            entry
                .set_password(&password)
                .map_err(KeyringAuthenticationStorageError::from)
        })?;

        Ok(())
    }

    fn get(&self, host: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        if !keyring_enabled() {
            return Ok(None);
        }

        let store_key = self.store_key.clone();
        let host_owned = host.to_string();

        // A timeout here means the keyring is unresponsive; fall back gracefully
        // to "no credentials found" so the other backends still get a chance.
        let p_string = match run_with_timeout("get", move || {
            let entry = open_entry(&store_key, &host_owned)?;
            match entry.get_password() {
                Ok(password) => Ok(Some(password)),
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(e) => Err(KeyringAuthenticationStorageError::from(e)),
            }
        }) {
            Ok(Some(password)) => password,
            // A timeout is treated like "no credentials found" so the lookup
            // falls through to the other backends.
            Ok(None) | Err(KeyringAuthenticationStorageError::Timeout { .. }) => return Ok(None),
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

    fn list(&self) -> Result<Vec<(String, Authentication)>, AuthenticationStorageError> {
        if !keyring_enabled() {
            return Ok(Vec::new());
        }

        let store_key = self.store_key.clone();

        let results = match run_with_timeout("list", move || {
            let store = credential_store()?;
            let spec = search_spec(&store_key);
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
                if service != store_key {
                    continue;
                }

                let password = match entry.get_password() {
                    Ok(password) => password,
                    Err(keyring_core::Error::NoEntry) => continue,
                    Err(err) => return Err(KeyringAuthenticationStorageError::from(err)),
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
        }) {
            Ok(results) => results,
            Err(KeyringAuthenticationStorageError::Timeout { .. }) => Vec::new(),
            Err(e) => return Err(e.into()),
        };

        Ok(results)
    }

    /// Enumerate stored hosts without reading their passwords. On macOS this
    /// avoids one keychain ACL prompt per entry — important for callers like
    /// the `auth logout` interactive picker that only need host metadata to
    /// build their UI.
    fn list_keys(&self) -> Result<Vec<String>, AuthenticationStorageError> {
        if !keyring_enabled() {
            return Ok(Vec::new());
        }

        let store_key = self.store_key.clone();

        let hosts = match run_with_timeout("list_keys", move || {
            let store = credential_store()?;
            let spec = search_spec(&store_key);
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
                if service != store_key {
                    continue;
                }
                hosts.push(account);
            }
            hosts.sort();
            Ok(hosts)
        }) {
            Ok(hosts) => hosts,
            Err(KeyringAuthenticationStorageError::Timeout { .. }) => Vec::new(),
            Err(e) => return Err(e.into()),
        };

        Ok(hosts)
    }

    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError> {
        if !keyring_enabled() {
            return Err(KeyringAuthenticationStorageError::Disabled.into());
        }

        let store_key = self.store_key.clone();
        let host = host.to_string();

        run_with_timeout("delete", move || {
            let entry = open_entry(&store_key, &host)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(err) => Err(KeyringAuthenticationStorageError::from(err)),
            }
        })?;

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

    /// With a timeout configured, a fast operation returns its result and does
    /// not latch the keyring off.
    #[test]
    fn run_with_timeout_returns_fast_result_without_latching() {
        let latch = AtomicBool::new(false);
        let result: Result<u32, _> =
            run_with_timeout_inner(Some(Duration::from_secs(30)), &latch, "test", || Ok(42));
        assert!(matches!(result, Ok(42)));
        assert!(!latch.load(Ordering::Relaxed), "a fast op must not latch");
    }

    /// An operation that blocks past the timeout returns `Timeout` and latches
    /// the keyring off so later operations can be skipped cheaply.
    #[test]
    fn run_with_timeout_times_out_and_latches() {
        let latch = AtomicBool::new(false);
        let result: Result<u32, _> =
            run_with_timeout_inner(Some(Duration::from_millis(50)), &latch, "test", || {
                // Simulate an unresponsive D-Bus call.
                thread::sleep(Duration::from_secs(30));
                Ok(0)
            });
        assert!(matches!(
            result,
            Err(KeyringAuthenticationStorageError::Timeout { operation: "test" })
        ));
        assert!(
            latch.load(Ordering::Relaxed),
            "a timeout must latch the keyring off"
        );
    }

    /// `None` runs the closure inline (original blocking behavior), never
    /// latching regardless of how long it takes.
    #[test]
    fn run_with_timeout_none_runs_inline() {
        let latch = AtomicBool::new(false);
        let result: Result<u32, _> = run_with_timeout_inner(None, &latch, "test", || Ok(7));
        assert!(matches!(result, Ok(7)));
        assert!(!latch.load(Ordering::Relaxed));
    }

    #[test]
    fn is_truthy_recognizes_enabled_values() {
        for value in ["1", "true", "TRUE", "Yes", " yes "] {
            assert!(is_truthy(value), "{value:?} should be truthy");
        }
        for value in ["0", "false", "no", "", "  "] {
            assert!(!is_truthy(value), "{value:?} should not be truthy");
        }
    }

    #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
    #[test]
    fn timeout_from_env_value_interprets_override() {
        assert_eq!(
            timeout_from_env_value(None),
            Some(DEFAULT_KEYRING_TIMEOUT),
            "unset falls back to the default"
        );
        assert_eq!(
            timeout_from_env_value(Some("0")),
            None,
            "0 disables the timeout"
        );
        assert_eq!(
            timeout_from_env_value(Some("42")),
            Some(Duration::from_secs(42))
        );
        assert_eq!(
            timeout_from_env_value(Some("  7 ")),
            Some(Duration::from_secs(7)),
            "surrounding whitespace is ignored"
        );
        assert_eq!(
            timeout_from_env_value(Some("not-a-number")),
            Some(DEFAULT_KEYRING_TIMEOUT),
            "invalid values fall back to the default"
        );
    }
}
