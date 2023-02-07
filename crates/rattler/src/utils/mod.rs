use std::{fmt::Write, path::PathBuf};
use url::Url;

pub use encoding::{AsyncEncoding, Encoding};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::Path;

mod encoding;

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

/// Compute the SHA256 hash of the file at the specified location.
pub fn compute_file_sha256(
    path: &Path,
) -> Result<sha2::digest::Output<sha2::Sha256>, std::io::Error> {
    // Open the file for reading
    let mut file = File::open(path)?;

    // Determine the hash of the file on disk
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;

    Ok(hasher.finalize())
}

#[cfg(test)]
mod test {
    use super::{compute_file_sha256, url_to_cache_filename};
    use rstest::rstest;
    use url::Url;

    #[rstest]
    #[case(
        "1234567890",
        "c775e7b757ede630cd0aa1113bd102661ab38829ca52a6422ab782862f268646"
    )]
    #[case(
        "Hello, world!",
        "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
    )]
    fn test_compute_file_sha256(#[case] input: &str, #[case] expected_hash: &str) {
        // Write a known value to a temporary file and verify that the compute hash matches what we would
        // expect.

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test");
        std::fs::write(&file_path, input).unwrap();
        let hash = compute_file_sha256(&file_path).unwrap();

        assert_eq!(format!("{hash:x}"), expected_hash)
    }

    #[test]
    fn test_url_to_cache_filename() {
        assert_eq!(
            url_to_cache_filename(&Url::parse("http://test.com/1234/").unwrap()),
            "302f0a61"
        );
    }
}
