//! Functionality to stream and extract packages directly from a [`reqwest::Url`] within a [`tokio`]
//! async context.

use crate::{ExtractError, ExtractResult};
use futures_util::stream::TryStreamExt;
use rattler_conda_types::package::ArchiveType;
use rattler_networking::AuthenticatedClient;
use reqwest::Response;
use std::path::Path;
use tokio::io::BufReader;
use tokio_util::either::Either;
use tokio_util::io::StreamReader;
use url::Url;

async fn get_reader(
    url: Url,
    client: AuthenticatedClient,
) -> Result<impl tokio::io::AsyncRead, ExtractError> {
    if url.scheme() == "file" {
        let file = tokio::fs::File::open(url.to_file_path().expect("..."))
            .await
            .map_err(ExtractError::IoError)?;

        Ok(Either::Left(BufReader::new(file)))
    } else {
        // Send the request for the file
        let response = client
            .get(url.clone())
            .send()
            .await
            .and_then(Response::error_for_status)
            .map_err(ExtractError::ReqwestError)?;

        // Get the response as a stream
        Ok(Either::Right(StreamReader::new(
            response
                .bytes_stream()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
        )))
    }
}

/// Extracts the contents a `.tar.bz2` package archive from the specified remote location.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use url::Url;
/// use rattler_networking::AuthenticatedClient;
/// use rattler_package_streaming::reqwest::tokio::extract_tar_bz2;
/// let _ = extract_tar_bz2(
///     AuthenticatedClient::default(),
///     Url::parse("https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2").unwrap(),
///     Path::new("/tmp"))
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_tar_bz2(
    client: AuthenticatedClient,
    url: Url,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    let reader = get_reader(url.clone(), client).await?;
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
/// use rattler_networking::AuthenticatedClient;
/// use url::Url;
/// let _ = extract_conda(
///     AuthenticatedClient::default(),
///     Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap(),
///     Path::new("/tmp"))
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_conda(
    client: AuthenticatedClient,
    url: Url,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // The `response` is used to stream in the package data
    let reader = get_reader(url.clone(), client).await?;
    crate::tokio::async_read::extract_conda(reader, destination).await
}

/// Extracts the contents a package archive from the specified remote location. The type of package
/// is determined based on the path of the url.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use url::Url;
/// use rattler_package_streaming::reqwest::tokio::extract;
/// use rattler_networking::AuthenticatedClient;
/// let _ = extract(
///     AuthenticatedClient::default(),
///     Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap(),
///     Path::new("/tmp"))
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract(
    client: AuthenticatedClient,
    url: Url,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    match ArchiveType::try_from(Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?
    {
        ArchiveType::TarBz2 => extract_tar_bz2(client, url, destination).await,
        ArchiveType::Conda => extract_conda(client, url, destination).await,
    }
}
