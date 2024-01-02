//! This module contains the authentication storage backend trait and implementations
use self::authentication::Authentication;
use anyhow::Result;

pub mod authentication;
pub mod backends;
pub mod storage;

/// A trait that defines the interface for authentication storage backends
pub trait StorageBackend: std::fmt::Debug {
    /// Store the given authentication information for the given host
    fn store(&self, host: &str, authentication: &Authentication) -> Result<()>;

    /// Retrieve the authentication information for the given host
    fn get(&self, host: &str) -> Result<Option<Authentication>>;

    /// Delete the authentication information for the given host
    fn delete(&self, host: &str) -> Result<()>;
}
