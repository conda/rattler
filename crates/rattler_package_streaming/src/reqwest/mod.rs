//! Functionality to stream and extract packages directly from a [`reqwest::Url`].
pub mod fetch;
pub mod sparse;
pub mod tokio;

#[cfg(test)]
mod test_server;
