//! Fetch individual files from remote `.conda` packages using HTTP range requests.
//!
//! Uses range requests to fetch only the info section, then stream-decompresses
//! the zstd tar archive only until the target file is found. This avoids
//! downloading the full package and avoids decompressing more data than needed.
//!
//! Only `.conda` archives on servers that support range requests are supported.
//! For a higher-level API that falls back to full downloads, see
//! [`super::fetch::fetch_package_file_from_url`].
//!
//! # Example
//!
//! ```rust,no_run
//! # #[tokio::main]
//! # async fn main() {
//! use rattler_conda_types::package::IndexJson;
//! use rattler_package_streaming::reqwest::sparse::fetch_package_file_sparse;
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//!
//! let client = ClientWithMiddleware::from(Client::new());
//! let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap();
//!
//! let index_json: IndexJson = fetch_package_file_sparse(client, url).await.unwrap();
//! # }
//! ```

use std::io::Cursor;
use std::path::Path;

use async_http_range_reader::{AsyncHttpRangeReader, CheckSupportMethod};
use async_zip::base::read::seek::ZipFileReader;
use futures::io::AsyncReadExt as _;
use http::HeaderMap;
use rattler_conda_types::package::{CondaArchiveType, PackageFile};
use reqwest_middleware::ClientWithMiddleware;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::debug;
use url::Url;

use crate::seek::get_file_from_archive;
use crate::ExtractError;

/// Default number of bytes to fetch from the end of the file.
/// 64KB should be enough for most packages to include the EOCD, Central Directory,
/// and often the entire info archive.
const DEFAULT_TAIL_SIZE: u64 = 64 * 1024;

/// Fetch the raw bytes of a single file from a remote `.conda` package's info
/// section using HTTP range requests.
///
/// Only decompresses the zstd stream until the target file is found, avoiding
/// unnecessary work for files early in the tar archive.
///
/// Returns an error if the URL does not point to a `.conda` archive, the server
/// does not support range requests, or the file is not found.
pub async fn fetch_file_from_remote_conda(
    client: ClientWithMiddleware,
    url: Url,
    target_path: &Path,
) -> Result<Vec<u8>, ExtractError> {
    debug!("fetching {:?} from remote archive {}", target_path, url);

    let archive_type = CondaArchiveType::try_from(std::path::Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?;

    if archive_type != CondaArchiveType::Conda {
        return Err(ExtractError::UnsupportedArchiveType);
    }

    // Create range reader (fetches last 64KB on construction)
    let (reader, _headers) = AsyncHttpRangeReader::new(
        client,
        url,
        CheckSupportMethod::NegativeRangeRequest(DEFAULT_TAIL_SIZE),
        HeaderMap::default(),
    )
    .await?;

    // Wrap for async_zip (tokio → futures traits + buffering)
    let buf_reader = futures::io::BufReader::new(reader.compat());

    // Open ZIP (parses EOCD + central directory, data already cached from range request)
    let mut zip_reader = ZipFileReader::new(buf_reader).await?;

    // Find the info-*.tar.zst entry
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

    // Read the compressed entry
    let mut entry_reader = zip_reader.reader_without_entry(index).await?;
    let mut compressed_data = Vec::new();
    entry_reader.read_to_end(&mut compressed_data).await?;

    // Stream-decompress zstd into tar and extract only the target file.
    // This stops decompressing as soon as the file is found.
    debug!(
        "decompressing zstd info archive ({} bytes) to find {:?}",
        compressed_data.len(),
        target_path
    );
    let decoder =
        zstd::Decoder::new(Cursor::new(&compressed_data)).map_err(ExtractError::IoError)?;
    let mut archive = tar::Archive::new(decoder);
    get_file_from_archive(&mut archive, target_path)
}

/// Fetch and parse a typed [`PackageFile`] from a remote `.conda` package
/// using HTTP range requests.
///
/// Only fetches the info section and decompresses only until the target file
/// is found. Returns an error if the server does not support range requests
/// or the package is not a `.conda` file.
///
/// # Example
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// use rattler_conda_types::package::IndexJson;
/// use rattler_package_streaming::reqwest::sparse::fetch_package_file_sparse;
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// use url::Url;
///
/// let client = ClientWithMiddleware::from(Client::new());
/// let url = Url::parse("https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda").unwrap();
///
/// let index_json: IndexJson = fetch_package_file_sparse(client, url).await.unwrap();
/// println!("Package: {}", index_json.name.as_normalized());
/// # }
/// ```
pub async fn fetch_package_file_sparse<P: PackageFile>(
    client: ClientWithMiddleware,
    url: Url,
) -> Result<P, ExtractError> {
    let bytes = fetch_file_from_remote_conda(client, url, P::package_path()).await?;
    P::from_str(&String::from_utf8_lossy(&bytes))
        .map_err(|e| ExtractError::ArchiveMemberParseError(P::package_path().to_owned(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reqwest::test_server;
    use std::path::PathBuf;

    fn test_file() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-fd-1-0.1.0-h4616a5c_0.conda")
    }

    #[tokio::test]
    async fn test_fetch_package_file_sparse() {
        use rattler_conda_types::package::{AboutJson, IndexJson};

        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let index_json: IndexJson = fetch_package_file_sparse(client.clone(), url.clone())
            .await
            .unwrap();
        insta::assert_yaml_snapshot!(index_json);

        let about_json: AboutJson = fetch_package_file_sparse(client, url).await.unwrap();
        insta::assert_yaml_snapshot!(about_json);
    }

    #[tokio::test]
    async fn test_fetch_raw_file() {
        let url = test_server::serve_file(test_file()).await;
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let raw = fetch_file_from_remote_conda(client, url, Path::new("info/index.json"))
            .await
            .unwrap();
        assert!(!raw.is_empty());
    }
}
