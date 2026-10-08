//! Shared helpers for unit tests across this crate's modules.

use crate::{AzureChannelUrl, AzureEndpointKey};

pub(crate) fn channel(url: &str) -> AzureChannelUrl {
    AzureChannelUrl::parse(url).unwrap_or_else(|err| panic!("{url} should parse: {err}"))
}

pub(crate) fn key(written: &str) -> AzureEndpointKey {
    AzureEndpointKey::parse(written)
        .unwrap_or_else(|err| panic!("{written} should parse as a key: {err}"))
}
