//! Parsing Azure Blob channel URLs and minting short-lived credentials for them.
//!
//! # Endpoint model
//!
//! The default grant is [`Auth::Anonymous`], so naming an endpoint in a URL by
//! itself sends nothing to it. Signing and sending live in `rattler_networking`,
//! not here.
//!
//! Userinfo (`user:pass@host`) is rejected wherever a host is parsed:
//! `az://real.host@evil.example/…` reads as the real host while addressing the
//! attacker's and is an invalid blob endpoint.

#[cfg(feature = "opendal")]
mod azblob;
mod channel_url;
#[cfg(feature = "clap")]
pub mod clap;
mod credentials;
mod endpoint_key;
mod error;
mod host;
mod locate;
mod names;
pub mod options;
#[cfg(feature = "clap")]
mod sas;
#[cfg(test)]
mod test_support;

#[cfg(feature = "opendal")]
pub use azblob::azblob_config;
pub use channel_url::AzureChannelUrl;
pub use credentials::AzureCredentials;
pub use endpoint_key::{AccountHost, AccountPath, AzureEndpointKey};
pub use error::AzureUrlError;
pub use host::AzureHost;
pub use locate::{AzureAddressing, AzureLocation, locate, locate_as};
pub use names::{AccountName, ContainerName};
pub use options::{Auth, AzureEndpointOptions, AzureFetchOptions, AzureScheme};
#[cfg(feature = "clap")]
pub use sas::{AzureCliSasError, mint_user_delegation_sas};

pub use secrecy::{ExposeSecret, SecretString};
