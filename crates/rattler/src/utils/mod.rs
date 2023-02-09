pub use encoding::{AsyncEncoding, Encoding};
pub use hash::{compute_file_sha256, HashingWriter, Sha256HashingWriter};
use std::{
    fmt::Write,
    path::PathBuf
};
use url::Url;

mod encoding;

mod hash;
#[cfg(test)]
pub(crate) mod simple_channel_server;

/// Returns the default cache directory used by rattler.
pub fn default_cache_dir() -> anyhow::Result<PathBuf> {
    Ok(dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory for current platform"))?
        .join("rattler/cache"))
}

/// Convert a URL to a cache filename
pub fn url_to_cache_filename(url: &Url) -> String {
    // Start Rant:
    // This function mimics behavior from Mamba which itself mimics this behavior from Conda.
    // However, I find this function absolutely ridiculous, it contains all sort of weird edge
    // cases and returns a hash that could very easily collide with other files. And why? Why not
    // simply return a little more descriptive? Like a URL encoded string?
    // End Rant.
    let mut url_str = url.to_string();

    // Ensure there is a slash if the URL is empty or doesnt refer to json file
    if url_str.is_empty() || (!url_str.ends_with('/') && !url_str.ends_with(".json")) {
        url_str.push('/')
    }

    // Mimicking conda's (weird) behavior by special handling repodata.json
    let url_str = url_str.strip_suffix("/repodata.json").unwrap_or(&url_str);

    // Compute the MD5 hash of the resulting URL string
    let hash = extendhash::md5::compute_hash(url_str.as_bytes());

    // Convert the hash to an MD5 hash.
    let mut result = String::with_capacity(8);
    for x in &hash[0..4] {
        write!(result, "{:02x}", x).unwrap();
    }
    result
}

#[cfg(test)]
mod test {
    use super::url_to_cache_filename;
    use url::Url;

    #[test]
    fn test_url_to_cache_filename() {
        assert_eq!(
            url_to_cache_filename(&Url::parse("http://test.com/1234/").unwrap()),
            "302f0a61"
        );
    }
}
