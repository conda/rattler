//! Parsing Azure Blob channel URLs and minting short-lived credentials for them.
//!
//! # Channel URLs
//!
//! A channel is written `az://host[:port]/…` and parsed into an
//! [`AzureChannelUrl`], which validates and normalizes the spelling so
//! equivalent URLs compare equal.
//!
//! # Addressing
//!
//! The same URL shape covers two layouts: host-style, where the account is a
//! label of the host (`az://acct.blob.core.windows.net/container/…`), and
//! path-style, where it is the first path segment
//! (`az://azurite.local:10000/acct/container/…`). [`locate`] resolves which
//! one a URL uses by matching it against the configured endpoint keys, and
//! [`locate_as`] lets a caller state the addressing outright.
//!
//! # Endpoint options
//!
//! Per-endpoint configuration ([`AzureEndpointOptions`]) is keyed by
//! [`AzureEndpointKey`], a host for host-style endpoints, `host/account` for
//! path-style ones. Two keys may share a host (`proxy.internal` and
//! `proxy.internal/accta`); [`locate`] matches the longest.

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
