//! Shared helpers for unit tests across this crate's modules.

use crate::AzureChannelUrl;

pub(crate) fn channel(url: &str) -> AzureChannelUrl {
    AzureChannelUrl::parse(url).unwrap_or_else(|err| panic!("{url} should parse: {err}"))
}
