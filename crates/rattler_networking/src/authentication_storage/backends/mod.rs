//! Multiple backends for storing authentication data.

pub mod file;
pub mod keyring;
pub mod memory;
pub mod netrc;
