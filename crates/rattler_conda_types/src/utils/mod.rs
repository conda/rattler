//! This module contains utility functions for url and serde

pub(crate) mod path;
pub(crate) mod serde;
pub(crate) mod url;
pub mod url_with_trailing_slash;

pub use self::path::{InvalidPathComponentError, ensure_safe_path_component};
pub use self::serde::TimestampMs;
pub(crate) use url_with_trailing_slash::UrlWithTrailingSlash;
