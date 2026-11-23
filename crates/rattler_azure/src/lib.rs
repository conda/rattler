//! Parsing and locating Azure Blob channels.
//!
//! # Channel URLs
//!
//! A channel is written `az://host[:port]/…` and parsed into an
//! [`AzureChannelUrl`], which validates and normalizes the spelling so
//! equivalent URLs compare equal.
//!
//! # Locating
//!
//! [`locate`] resolves a channel URL to an [`AzureEndpointKey`], written
//! `<host>` or `<host>/<account>`, and the container that follows it, if any.

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
pub use locate::{AzureLocation, KeySource, KeyedLocation, UnkeyedReason, locate};
pub use names::{AccountName, ContainerName};
pub use options::{Auth, AzureEndpointOptions, AzureFetchOptions, AzureScheme};
