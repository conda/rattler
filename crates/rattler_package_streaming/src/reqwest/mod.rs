//! Functionality to stream and extract packages directly from a [`reqwest::Url`].

#[cfg(feature = "tokio")]
pub mod tokio;

#[cfg(feature = "blocking")]
pub mod blocking;
#[cfg(feature = "blocking")]
pub use blocking::{extract, extract_conda, extract_tar_bz2};
