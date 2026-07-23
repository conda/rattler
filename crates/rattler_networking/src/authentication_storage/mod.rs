//! This module contains the authentication storage backend trait and implementations
use self::authentication::Authentication;

pub mod authentication;
pub mod backends;
pub mod storage;

/// An error occurred when accessing the authentication storage
#[derive(thiserror::Error, Debug)]
pub enum AuthenticationStorageError {
    /// An error occurred when accessing the file storage
    #[error("FileStorageError")]
    FileStorageError(#[from] crate::authentication_storage::backends::file::FileStorageError),
    /// An error occurred when accessing the keyring storage
    #[cfg(feature = "keyring")]
    #[error("KeyringStorageError")]
    KeyringStorageError(
        #[from] crate::authentication_storage::backends::keyring::KeyringAuthenticationStorageError,
    ),
    /// An error occurred when accessing the netrc storage
    #[cfg(feature = "netrc-rs")]
    #[error("NetRcStorageError")]
    NetRcStorageError(#[from] crate::authentication_storage::backends::netrc::NetRcStorageError),
    /// An error occurred when accessing the memory storage
    #[error("MemoryStorageError")]
    MemoryStorageError(#[from] crate::authentication_storage::backends::memory::MemoryStorageError),
}

/// A trait that defines the interface for authentication storage backends
pub trait StorageBackend: std::fmt::Debug {
    /// A short human-readable description identifying this backend (and any
    /// relevant location, like a file path). Surfaced to users by the
    /// `auth status` CLI so they can tell where each credential lives.
    fn name(&self) -> String;

    /// Store the given authentication information for the given host
    fn store(
        &self,
        host: &str,
        authentication: &Authentication,
    ) -> Result<(), AuthenticationStorageError>;

    /// Retrieve the authentication information for the given host
    fn get(&self, host: &str) -> Result<Option<Authentication>, AuthenticationStorageError>;

    /// List all authentication entries known to this backend.
    ///
    /// Some backends, such as platform keyrings, cannot enumerate arbitrary
    /// legacy entries. They may return only entries that were stored with an
    /// index maintained by this crate.
    fn list(&self) -> Result<Vec<(String, Authentication)>, AuthenticationStorageError> {
        Ok(Vec::new())
    }

    /// List the host keys this backend holds, *without* decrypting or fetching
    /// stored secrets.
    ///
    /// The default implementation falls back to [`list`](Self::list), which is
    /// expensive on backends like the macOS keychain where reading each
    /// credential triggers a per-item ACL prompt. Backends that can enumerate
    /// metadata cheaply should override this — that lets callers (e.g. an
    /// interactive picker) show entries to the user without prompting for
    /// every stored password.
    fn list_keys(&self) -> Result<Vec<String>, AuthenticationStorageError> {
        Ok(self.list()?.into_iter().map(|(host, _)| host).collect())
    }

    /// Delete the authentication information for the given host
    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError>;
}
