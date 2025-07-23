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
    /// Store the given authentication information for the given host
    fn store(
        &self,
        host: &str,
        authentication: &Authentication,
    ) -> Result<(), AuthenticationStorageError>;

    /// Retrieve the authentication information for the given host
    fn get(&self, host: &str) -> Result<Option<Authentication>, AuthenticationStorageError>;

    /// Delete the authentication information for the given host
    fn delete(&self, host: &str) -> Result<(), AuthenticationStorageError>;
}
