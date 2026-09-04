//! Shared helpers for unit tests across this crate's modules.

use crate::{AzureChannelUrl, AzureEndpointKey, AzureLocation, ContainerName, locate};

pub(crate) fn channel(url: &str) -> AzureChannelUrl {
    AzureChannelUrl::parse(url).unwrap_or_else(|err| panic!("{url} should parse: {err}"))
}

pub(crate) fn key(written: &str) -> AzureEndpointKey {
    AzureEndpointKey::parse(written)
        .unwrap_or_else(|err| panic!("{written} should parse as a key: {err}"))
}

pub(crate) fn located(url: &str, configured: &[&str]) -> AzureLocation {
    let configured = configured.iter().copied().map(key).collect::<Vec<_>>();
    locate(&channel(url), |candidate| configured.contains(candidate))
        .unwrap_or_else(|err| panic!("{url} should locate: {err}"))
}

pub(crate) fn container(name: &str) -> ContainerName {
    ContainerName::new(name).expect("test container name")
}

#[cfg(feature = "opendal")]
pub(crate) fn path_style(url: &str) -> AzureLocation {
    crate::locate_as(&channel(url), crate::AzureAddressing::PathStyle)
        .unwrap_or_else(|err| panic!("{url} should locate path-style: {err}"))
}

pub(crate) fn hash_of(value: &impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
