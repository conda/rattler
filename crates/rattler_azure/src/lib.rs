//! Parsing Azure Blob channel URLs.
//!
//! # Channel URLs
//!
//! A channel is written `az://host[:port]/…` and parsed into an
//! [`AzureChannelUrl`], which validates and normalizes the spelling so
//! equivalent URLs compare equal.
//!
//! [`locate`] resolves a channel URL to an [`AzureEndpointKey`] and container.

mod channel_url;
mod endpoint_key;
mod error;
mod host;
mod locate;
mod names;
pub mod options;
#[cfg(test)]
mod test_support;

pub use channel_url::AzureChannelUrl;
pub use endpoint_key::AzureEndpointKey;
pub use error::AzureUrlError;
pub use host::AzureHost;
pub use locate::{AzureLocation, KeySource, KeyedLocation, locate};
pub use names::{AccountName, ContainerName};
pub use options::{Auth, AzureEndpointOptions, AzureFetchOptions, AzureScheme};
