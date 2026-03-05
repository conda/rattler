//! Functionality to fetch package metadata from a remote `.conda` package using HTTP range requests.
//!
//! This module allows fetching just the `info/` section of a `.conda` package without downloading
//! the entire file. This is achieved by using HTTP Range requests to fetch only the necessary
//! bytes from the end of the zip archive.
//!
//! # Example
//!
//! ```rust,no_run
//! # #[tokio::main]
//! # async fn main() {
//! use rattler_conda_types::package::IndexJson;
//! use rattler_package_streaming::reqwest::range::fetch_package_file_from_url;
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//!
//! let client = ClientWithMiddleware::from(Client::new());
//! let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
//!
//! let index_json: IndexJson = fetch_package_file_from_url(client, url)
//!     .await
//!     .unwrap();
//!
//! println!("Package: {}", index_json.name.as_normalized());
//! # }
//! ```

use std::io::{Cursor, Read};

use async_http_range_reader::{
    AsyncHttpRangeReader, AsyncHttpRangeReaderError, CheckSupportMethod,
};
use async_zip::base::read::seek::ZipFileReader;
use futures::io::AsyncReadExt as _;
use http::HeaderMap;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tar::Archive;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::debug;
use url::Url;

use crate::seek::read_package_file_content;
use crate::ExtractError;

/// Default number of bytes to fetch from the end of the file.
/// 64KB should be enough for most packages to include the EOCD, Central Directory,
/// and often the entire info archive.
const DEFAULT_TAIL_SIZE: u64 = 64 * 1024;

/// Extract a file from a tar archive bytes.
fn extract_file_from_tar<P: PackageFile>(tar_bytes: &[u8]) -> Result<P, ExtractError> {
    let cursor = Cursor::new(tar_bytes);
    let mut archive = Archive::new(cursor);

    let target_path = P::package_path();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;

        if path == target_path {
            let mut contents = String::new();
            entry.read_to_string(&mut contents)?;
            return P::from_str(&contents)
                .map_err(|e| ExtractError::ArchiveMemberParseError(target_path.to_path_buf(), e));
        }
    }

    Err(ExtractError::MissingComponent)
}

/// Fetch a specific [`PackageFile`] from a remote package using HTTP range requests.
///
/// This function fetches only the minimal bytes needed from the package, typically
/// just the `info/` section which is located at the end of the `.conda` archive.
///
/// If the server returns 405 (Method Not Allowed) or the package is not a `.conda` file,
/// the function falls back to downloading the entire package.
///
/// # Arguments
///
/// * `client` - The HTTP client to use for requests
/// * `url` - The URL of the package
///
/// # Returns
///
/// The parsed package file (e.g., `IndexJson`, `AboutJson`, etc.)
///
/// # Example
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// use rattler_conda_types::package::IndexJson;
/// use rattler_package_streaming::reqwest::range::fetch_package_file_from_url;
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// use url::Url;
///
/// let client = ClientWithMiddleware::from(Client::new());
/// let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
///
/// let index_json: IndexJson = fetch_package_file_from_url(client, url)
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn fetch_package_file_from_url<P: PackageFile>(
    client: ClientWithMiddleware,
    url: Url,
) -> Result<P, ExtractError> {
    debug!(
        "fetching {} from {} using range requests",
        P::package_path().display(),
        url
    );

    // Determine archive type from URL - only .conda supports efficient range requests
    let archive_type = CondaArchiveType::try_from(std::path::Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?;

    if archive_type != CondaArchiveType::Conda {
        debug!("archive type is .tar.bz2, falling back to full download");
        return fetch_package_file_full_download(&client, &url, archive_type).await;
    }

    // Step 1: Create range reader (fetches last 64KB on construction)
    let (reader, _headers) = match AsyncHttpRangeReader::new(
        client.clone(),
        url.clone(),
        CheckSupportMethod::NegativeRangeRequest(DEFAULT_TAIL_SIZE),
        HeaderMap::default(),
    )
    .await
    {
        Ok(r) => r,
        Err(AsyncHttpRangeReaderError::HttpRangeRequestUnsupported) => {
            debug!("server does not support range requests, falling back to full download");
            return fetch_package_file_full_download(&client, &url, archive_type).await;
        }
        Err(e) => return Err(e.into()),
    };

    // Step 2: Wrap for async_zip (tokio → futures traits + buffering)
    let buf_reader = futures::io::BufReader::new(reader.compat());

    // Step 3: Open ZIP (parses EOCD + central directory, data already cached from step 1)
    let mut zip_reader = ZipFileReader::new(buf_reader).await?;

    // Step 4: Find the info-*.tar.zst entry
    let (index, _entry) = zip_reader
        .file()
        .entries()
        .iter()
        .enumerate()
        .find(|(_, e)| {
            e.filename()
                .as_str()
                .map(|f| f.starts_with("info-") && f.ends_with(".tar.zst"))
                .unwrap_or(false)
        })
        .ok_or(ExtractError::MissingComponent)?;

    // Step 5: Read the entry (async_zip handles seek + local header parsing)
    let mut entry_reader = zip_reader.reader_without_entry(index).await?;
    let mut compressed_data = Vec::new();
    entry_reader.read_to_end(&mut compressed_data).await?;

    // Step 6: Decompress zstd → extract from tar
    debug!(
        "decompressing {} bytes of zstd-compressed info archive",
        compressed_data.len()
    );
    let tar_bytes =
        zstd::decode_all(Cursor::new(&compressed_data)).map_err(ExtractError::IoError)?;
    debug!(
        "decompressed to {} bytes, extracting {}",
        tar_bytes.len(),
        P::package_path().display()
    );

    extract_file_from_tar::<P>(&tar_bytes)
}

/// Download full package and extract [`PackageFile`] when range requests fail.
async fn fetch_package_file_full_download<P: PackageFile>(
    client: &ClientWithMiddleware,
    url: &Url,
    archive_type: CondaArchiveType,
) -> Result<P, ExtractError> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(ExtractError::ReqwestError)?
        .error_for_status()
        .map_err(|e| ExtractError::ReqwestError(e.into()))?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ExtractError::ReqwestError(e.into()))?;

    let content = read_package_file_content(Cursor::new(&*bytes), archive_type, P::package_path())?;
    P::from_str(&String::from_utf8_lossy(&content))
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_index_json_from_conda_forge() {
        use rattler_conda_types::package::IndexJson;

        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        let url = Url::parse(
            "https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda",
        )
        .unwrap();

        let index_json: IndexJson = fetch_package_file_from_url(client, url).await.unwrap();

        assert_eq!(index_json.name.as_normalized(), "tzdata");
        assert_eq!(index_json.version.to_string(), "2024b");
    }

    #[tokio::test]
    async fn test_fetch_about_json_from_conda_forge() {
        use rattler_conda_types::package::AboutJson;

        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        let url = Url::parse(
            "https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda",
        )
        .unwrap();

        let about_json: AboutJson = fetch_package_file_from_url(client, url).await.unwrap();

        // tzdata package should have license info
        assert!(about_json.license.is_some());
    }
}
