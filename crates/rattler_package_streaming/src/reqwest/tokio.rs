//! Functionality to stream and extract packages directly from a [`reqwest::Url`] within a [`tokio`]
//! async context.

use crate::{ExtractError, ExtractResult};
use futures_util::StreamExt;
use rattler_conda_types::package::ArchiveType;
use rattler_networking::AuthenticatedClient;
use reqwest::Response;
use std::{
    fs::File,
    io::{BufReader, Read, Seek, Write},
    path::Path,
};
use tokio::task::JoinError;
use url::Url;

enum LocalOrTemp {
    Local(BufReader<File>),
    Temp(tempfile::SpooledTempFile),
}

impl Read for LocalOrTemp {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            LocalOrTemp::Local(file) => file.read(buf),
            LocalOrTemp::Temp(file) => file.read(buf),
        }
    }
}

async fn get_reader(
    url: Url,
    client: AuthenticatedClient,
) -> Result<impl Read + Send + 'static, ExtractError> {
    // If the url is a file path, then just open the file
    if url.scheme() == "file" {
        if let Ok(path) = url.to_file_path() {
            let file = File::open(path).map_err(ExtractError::IoError)?;
            return Ok(LocalOrTemp::Local(BufReader::new(file)));
        }
    }

    // Create a request for the file.
    let response = client
        .get(url.clone())
        .send()
        .await
        .and_then(Response::error_for_status)
        .map_err(ExtractError::ReqwestError)?;

    // Construct a spooled temporary file, a memory buffer that will be rolled over to disk if it
    // exceeds a certain size.
    let mut temp_file = tempfile::SpooledTempFile::new(5 * 1024 * 1024);

    // Stream the download to the temporary file
    let mut bytes = response.bytes_stream();
    while let Some(bytes) = bytes.next().await {
        let bytes = bytes.map_err(ExtractError::ReqwestError)?;
        temp_file.write_all(&bytes).map_err(ExtractError::IoError)?;
    }

    // Rewind the spooled file to the beginning and return
    temp_file.rewind().map_err(ExtractError::IoError)?;

    Ok(LocalOrTemp::Temp(temp_file))
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
    let destination = destination.to_owned();
    match tokio::task::spawn_blocking(move || crate::read::extract_tar_bz2(reader, &destination))
        .await
        .map_err(JoinError::try_into_panic)
    {
        Ok(result) => result,
        Err(Ok(panic)) => std::panic::resume_unwind(panic),
        Err(Err(_)) => Err(ExtractError::Cancelled),
    }
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
    let reader = get_reader(url.clone(), client).await?;
    let destination = destination.to_owned();
    match tokio::task::spawn_blocking(move || crate::read::extract_conda(reader, &destination))
        .await
        .map_err(JoinError::try_into_panic)
    {
        Ok(result) => result,
        Err(Ok(panic)) => std::panic::resume_unwind(panic),
        Err(Err(_)) => Err(ExtractError::Cancelled),
    }
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
