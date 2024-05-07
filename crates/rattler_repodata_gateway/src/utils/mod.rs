use ::url::Url;
pub use body::BodyStreamExt;
pub use encoding::{AsyncEncoding, Encoding};
pub use flock::LockedFile;
use std::fmt::Write;
use tokio::task::JoinError;

mod encoding;

#[cfg(test)]
pub(crate) mod simple_channel_server;

mod body;
mod flock;

/// Convert a URL to a cache filename
pub(crate) fn url_to_cache_filename(url: &Url) -> String {
    // Start Rant:
    // This function mimics behavior from Mamba which itself mimics this behavior from Conda.
    // However, I find this function absolutely ridiculous, it contains all sort of weird edge
    // cases and returns a hash that could very easily collide with other files. And why? Why not
    // simply return a little more descriptive? Like a URL encoded string?
    // End Rant.
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

/// A marker type that is used to signal that a task was cancelled.
pub(crate) struct Cancelled;

/// Run a blocking task to complettion. If the task is cancelled, the function
/// will return an error converted from `Error`.
pub async fn run_blocking_task<T, E, F>(f: F) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: From<Cancelled> + Send + 'static,
{
    match tokio::task::spawn_blocking(f)
        .await
        .map_err(JoinError::try_into_panic)
    {
        Ok(result) => result,
        Err(Err(_err)) => Err(E::from(Cancelled)),
        Err(Ok(payload)) => std::panic::resume_unwind(payload),
    }
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
