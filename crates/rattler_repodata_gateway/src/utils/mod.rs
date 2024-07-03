use std::fmt::Write;

use ::url::Url;
pub use body::BodyStreamExt;
pub use encoding::{AsyncEncoding, Encoding};
pub use flock::LockedFile;

mod encoding;

#[cfg(test)]
pub(crate) mod simple_channel_server;

mod body;
mod flock;

/// Convert a URL to a cache filename
pub(crate) fn url_to_cache_filename(url: &Url) -> String {
    // Start Rant:
    // This function mimics behavior from Mamba which itself mimics this behavior
    // from Conda. However, I find this function absolutely ridiculous, it
    // contains all sort of weird edge cases and returns a hash that could very
    // easily collide with other files. And why? Why not simply return a little
    // more descriptive? Like a URL encoded string? End Rant.
    let mut url_str = url.to_string();

    // Ensure there is a slash if the URL is empty or doesnt refer to json file
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
    use std::path::{Path, PathBuf};

    use tempfile::NamedTempFile;
    use url::Url;

    use super::url_to_cache_filename;

    #[test]
    fn test_url_to_cache_filename() {
        assert_eq!(
            url_to_cache_filename(&Url::parse("http://test.com/1234/").unwrap()),
            "302f0a61"
        );
    }

    pub(crate) fn test_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
    }

    pub(crate) async fn fetch_repo_data(subdir: &str) -> Result<(), reqwest::Error> {
        let path = test_dir().join(format!("channels/conda-forge/{subdir}/repodata.json"));

        // Early out if the file already eixsts
        if path.exists() {
            return Ok(());
        }

        // Create the parent directory if it doesn't exist
        let parent_dir = path.parent().unwrap();
        tokio::fs::create_dir_all(&parent_dir).await.unwrap();

        // Acquire a lock on the file to ensure we don't download the file twice.
        let mut lock = fslock::LockFile::open(&parent_dir.join(".lock")).unwrap();
        loop {
            if lock.try_lock_with_pid().unwrap() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // Early out if the file was downloaded while we were waiting for the lock
        if path.is_file() {
            return Ok(());
        }

        // Download the file and persist after download
        let mut file = NamedTempFile::new_in(parent_dir).unwrap();
        let data = reqwest::get(format!(
            "https://rattler-test.pixi.run/test-data/channels/conda-forge/{subdir}/repodata.json"
        ))
        .await?;
        tokio::fs::write(&mut file, data.bytes().await?)
            .await
            .unwrap();
        file.persist(&path).unwrap();

        Ok(())
    }
}
