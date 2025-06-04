//! Multiple backends for storing authentication data.

pub mod file;
#[cfg(feature = "keyring")]
pub mod keyring;
pub mod memory;
#[cfg(feature = "netrc")]
pub mod netrc;
