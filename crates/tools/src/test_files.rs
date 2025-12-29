use fs_err::tokio as tokio_fs;
use rattler_digest::Sha256;
use reqwest::blocking::Client;
use std::time::Instant;
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tempfile::NamedTempFile;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid url: {0}")]
    InvalidUrl(String),

    #[error("could not determine the systems cache directory")]
    FailedToDetermineCacheDir,

    #[error("failed to create temporary file")]
    FailedToCreateTemporaryFile(#[source] std::io::Error),

    #[error("failed to acquire cache lock")]
    FailedToAcquireCacheLock(#[source] std::io::Error),

    #[error("failed to create cache dir {0}")]
    FailedToCreateCacheDir(String, #[source] std::io::Error),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("hash mismatch. Expected: {0}, Actual: {1}")]
    HashMismatch(String, String),
}

/// Returns a [`Client`] that can be shared between all requests.
fn reqwest_client() -> Client {
    static CLIENT: OnceLock<Mutex<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| Mutex::new(Client::new()))
        .lock()
        .unwrap()
        .clone()
}

/// Returns the cache directory to use for storing cached files
fn cache_dir() -> Result<PathBuf, Error> {
    Ok(dirs::cache_dir()
        .ok_or(Error::FailedToDetermineCacheDir)?
        .join("rattler/tests/cache/"))
}

pub async fn download_and_cache_file_async(
    url: Url,
    expected_sha256: &str,
) -> Result<PathBuf, Error> {
    let hash = expected_sha256.to_string();
    tokio::task::spawn_blocking(move || download_and_cache_file(url, &hash))
        .await
        .unwrap()
}

/// Downloads a file to a semi-temporary location that can be used for testing.
pub fn download_and_cache_file(url: Url, expected_sha256: &str) -> Result<PathBuf, Error> {
    // Acquire a lock to the cache directory
    let cache_dir = cache_dir()?;

    // Determine the extension of the file
    let filename = url
        .path_segments()
        .into_iter()
        .flatten()
        .last()
        .ok_or_else(|| Error::InvalidUrl(String::from("missing filename")))?;

    // Determine the final location of the file
    let final_parent_dir = cache_dir.join(expected_sha256);
    let final_path = final_parent_dir.join(filename);

    // Ensure the cache directory exists
    std::fs::create_dir_all(&final_parent_dir)
        .map_err(|e| Error::FailedToCreateCacheDir(final_parent_dir.display().to_string(), e))?;

    // Acquire the lock on the cache directory
    let mut lock = fslock::LockFile::open(&cache_dir.join(".lock"))
        .map_err(Error::FailedToAcquireCacheLock)?;
    lock.lock_with_pid()
        .map_err(Error::FailedToAcquireCacheLock)?;

    // Check if the file is already there
    if final_path.is_file() {
        return Ok(final_path);
    }

    eprintln!("Downloading {} to {}", url, final_path.display());
    let start_download = Instant::now();

    // Construct a temporary file to which we will write the file
    let tempfile = tempfile::NamedTempFile::new_in(&final_parent_dir)
        .map_err(Error::FailedToCreateTemporaryFile)?;

    // Execute the download request
    let mut response = reqwest_client().get(url).send()?.error_for_status()?;

    // Compute the hash while downloading
    let mut writer = rattler_digest::HashingWriter::<_, Sha256>::new(tempfile);
    std::io::copy(&mut response, &mut writer)?;
    let (tempfile, hash) = writer.finalize();

    // Check if the hash matches
    let actual_hash = format!("{hash:x}");
    if actual_hash != expected_sha256 {
        return Err(Error::HashMismatch(expected_sha256.to_owned(), actual_hash));
    }

    // Write the file to its final destination
    tempfile.persist(&final_path).map_err(|e| e.error)?;

    let end_download = Instant::now();
    eprintln!(
        "Finished download in {}s",
        (end_download - start_download).as_secs_f32()
    );

    Ok(final_path)
}

/// Returns the path to the test data directory
pub fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}

/// Fetches conda-forge test repodata from rattler-test.pixi.run (blocking version).
///
/// This function downloads conda-forge repodata files from rattler-test.pixi.run if they
/// don't already exist locally. It uses file locking to ensure thread-safety and avoid
/// duplicate downloads.
///
/// # Arguments
/// * `subdir` - The subdirectory (platform) to fetch, e.g., "linux-64", "noarch"
///
/// # Returns
/// * `Ok(PathBuf)` - Path to the repodata.json file
/// * `Err(reqwest::Error)` if the download failed
pub fn fetch_test_conda_forge_repodata(subdir: &str) -> Result<PathBuf, reqwest::Error> {
    let path = test_data_dir().join(format!("channels/conda-forge/{subdir}/repodata.json"));

    // Early out if the file already exists
    if path.exists() {
        return Ok(path);
    }

    // Create the parent directory if it doesn't exist
    let parent_dir = path.parent().unwrap();
    std::fs::create_dir_all(parent_dir).unwrap();

    // Acquire a lock on the file to ensure we don't download the file twice.
    let mut lock = fslock::LockFile::open(&parent_dir.join(".lock")).unwrap();
    loop {
        if lock.try_lock_with_pid().unwrap() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Early out if the file was downloaded while we were waiting for the lock
    if path.is_file() {
        return Ok(path);
    }

    // Download the file and persist after download
    let mut file = NamedTempFile::new_in(parent_dir).unwrap();
    let mut response = reqwest_client()
        .get(format!(
            "https://rattler-test.pixi.run/test-data/channels/conda-forge/{subdir}/repodata.json"
        ))
        .send()?
        .error_for_status()?;

    std::io::copy(&mut response, &mut file).unwrap();
    file.persist(&path).unwrap();

    Ok(path)
}

/// Fetches conda-forge test repodata from rattler-test.pixi.run (async version).
///
/// This function downloads conda-forge repodata files from rattler-test.pixi.run if they
/// don't already exist locally. It uses file locking to ensure thread-safety and avoid
/// duplicate downloads.
///
/// # Arguments
/// * `subdir` - The subdirectory (platform) to fetch, e.g., "linux-64", "noarch"
///
/// # Returns
/// * `Ok(PathBuf)` - Path to the repodata.json file
/// * `Err(reqwest::Error)` if the download failed
pub async fn fetch_test_conda_forge_repodata_async(
    subdir: &str,
) -> Result<PathBuf, reqwest::Error> {
    let path = test_data_dir().join(format!("channels/conda-forge/{subdir}/repodata.json"));

    // Early out if the file already exists
    if path.exists() {
        return Ok(path);
    }

    // Create the parent directory if it doesn't exist
    let parent_dir = path.parent().unwrap();
    tokio_fs::create_dir_all(parent_dir).await.unwrap();

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
        return Ok(path);
    }

    // Download the file and persist after download
    let mut file = NamedTempFile::new_in(parent_dir).unwrap();
    let data = reqwest::get(format!(
        "https://rattler-test.pixi.run/test-data/channels/conda-forge/{subdir}/repodata.json"
    ))
    .await?;
    tokio_fs::write(&mut file, data.bytes().await?)
        .await
        .unwrap();
    file.persist(&path).unwrap();

    Ok(path)
}
