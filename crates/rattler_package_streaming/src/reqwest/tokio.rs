//! Functionality to stream and extract packages directly from a [`reqwest::Url`] within a [`tokio`]
//! async context.

use crate::ExtractError;
use futures_util::stream::TryStreamExt;
use rattler_conda_types::package::ArchiveType;
use reqwest::{Client, IntoUrl, Response};
use std::path::Path;
use tokio_util::io::StreamReader;

/// Extracts the contents a `.tar.bz2` package archive from the specified remote location.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use rattler_package_streaming::reqwest::tokio::extract_tar_bz2;
/// # use reqwest::Client;
/// let _ = extract_tar_bz2(
///     Client::default(),
///     "https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2",
///     Path::new("/tmp"))
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_tar_bz2(
    client: Client,
    url: impl IntoUrl,
    destination: &Path,
) -> Result<(), ExtractError> {
    // Send the request for the file
    let response = client
        .get(url)
        .send()
        .await
        .and_then(Response::error_for_status)
        .map_err(ExtractError::ReqwestError)?;

    // Get the response as a stream
    let reader = StreamReader::new(
        response
            .bytes_stream()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
    );

    // The `response` is used to stream in the package data
    crate::tokio::async_read::extract_tar_bz2(reader, destination).await
}

/// Extracts the contents a `.conda` package archive from the specified remote location.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use rattler_package_streaming::reqwest::tokio::extract_conda;
/// # use reqwest::Client;
/// let _ = extract_conda(
///     Client::default(),
///     "https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda",
///     Path::new("/tmp"))
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_conda(
    client: Client,
    url: impl IntoUrl,
    destination: &Path,
) -> Result<(), ExtractError> {
    // Send the request for the file
    let response = client
        .get(url)
        .send()
        .await
        .and_then(Response::error_for_status)
        .map_err(ExtractError::ReqwestError)?;

    // Get the response as a stream
    let reader = StreamReader::new(
        response
            .bytes_stream()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
    );

    // The `response` is used to stream in the package data
    crate::tokio::async_read::extract_conda(reader, destination).await
}

/// Extracts the contents a package archive from the specified remote location. The type of package
/// is determined based on the path of the url.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use rattler_package_streaming::reqwest::tokio::extract;
/// # use reqwest::Client;
/// let _ = extract(
///     Client::default(),
///     "https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda",
///     Path::new("/tmp"))
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract(
    client: Client,
    url: impl IntoUrl,
    destination: &Path,
) -> Result<(), ExtractError> {
    let url = url
        .into_url()
        .map_err(reqwest::Error::from)
        .map_err(ExtractError::ReqwestError)?;

    match ArchiveType::try_from(Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?
    {
        ArchiveType::TarBz2 => extract_tar_bz2(client, url, destination).await,
        ArchiveType::Conda => extract_conda(client, url, destination).await,
    }
}
