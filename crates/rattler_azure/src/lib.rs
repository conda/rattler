//! Parsing Azure Blob channel URLs and minting short-lived credentials for them.
//!
//! # Channel URLs
//!
//! A channel is written `az://host[:port]/…` and parsed into an
//! [`AzureChannelUrl`], which validates and normalizes the spelling so
//! equivalent URLs compare equal.

mod channel_url;
mod credentials;
mod error;
mod host;
mod names;
pub mod options;
#[cfg(test)]
mod test_support;

pub use channel_url::AzureChannelUrl;
pub use credentials::AzureCredentials;
pub use error::AzureUrlError;
pub use host::AzureHost;
pub use names::{AccountName, ContainerName};
pub use options::{Auth, AzureEndpointOptions, AzureFetchOptions, AzureScheme};

pub use secrecy::{ExposeSecret, SecretString};
