//! Utility modules providing various helper functionality.
//!
//! This module contains generic utilities and abstractions used throughout
//! the crate, including request deduplication, encoding handling, and file locking.

pub use body::BodyStreamExt;
pub use encoding::{AsyncEncoding, Encoding};

mod encoding;

#[cfg(test)]
pub(crate) mod simple_channel_server;

mod body;
#[cfg(not(target_arch = "wasm32"))]
mod flock;

#[cfg(not(target_arch = "wasm32"))]
pub use flock::LockedFile;

/// Convert a URL to a cache filename
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn url_to_cache_filename(url: &::url::Url) -> String {
    use std::fmt::Write;

    // Start Rant:
    // This function mimics behavior from Mamba which itself mimics this behavior
    // from Conda. However, I find this function absolutely ridiculous, it
    // contains all sort of weird edge cases and returns a hash that could very
    // easily collide with other files. And why? Why not simply return a little
    // more descriptive? Like a URL encoded string? End Rant.
    let mut url_str = url.to_string();

    // Ensure there is a slash if the URL is empty or doesn't refer to json file
    if url_str.is_empty() || (!url_str.ends_with('/') && !url_str.ends_with(".json")) {
        url_str.push('/');
    }

    // Mimicking conda's (weird) behavior by special handling repodata.json
    let url_str = url_str.strip_suffix("/repodata.json").unwrap_or(&url_str);

    // Compute the MD5 hash of the resulting URL string
    let hash = rattler_digest::compute_bytes_digest::<rattler_digest::Md5>(url_str);

    // Convert the hash to an MD5 hash.
    let mut result = String::with_capacity(8);
    for x in &hash[0..4] {
        write!(result, "{x:02x}").unwrap();
    }
    result
}

#[cfg(test)]
pub(crate) mod test {
    use url::Url;

    use super::url_to_cache_filename;

    #[test]
    fn test_url_to_cache_filename() {
        assert_eq!(
            url_to_cache_filename(&Url::parse("http://test.com/1234/").unwrap()),
            "302f0a61"
        );
    }
}
