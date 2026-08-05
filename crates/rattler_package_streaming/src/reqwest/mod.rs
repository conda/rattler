//! Functionality to stream and extract packages directly from a [`reqwest::Url`].
#[cfg(not(target_arch = "wasm32"))]
pub mod fetch;
pub mod full_download;
#[cfg(not(target_arch = "wasm32"))]
pub mod sparse;
pub mod tokio;

#[cfg(test)]
pub(crate) mod test_server;
