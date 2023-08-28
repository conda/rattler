//! Functionality to stream and extract packages directly from a [`reqwest::Url`] with a blocking client

use crate::{ExtractError, ExtractResult};
use rattler_conda_types::package::ArchiveType;
use rattler_networking::AuthenticatedClientBlocking;
use reqwest::blocking::Response;
use reqwest::IntoUrl;
use std::path::Path;

/// Extracts the contents a `.tar.bz2` package archive from the specified remote location.
///
/// ```rust,no_run
/// # use std::path::Path;
/// use rattler_package_streaming::reqwest::extract_tar_bz2;
/// use rattler_networking::AuthenticatedClientBlocking;
/// # use reqwest::blocking::Client;
/// let _ = extract_tar_bz2(
///     AuthenticatedClientBlocking::default(),
///     "https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2",
///     Path::new("/tmp"))
///     .unwrap();
/// ```
pub fn extract_tar_bz2(
    client: AuthenticatedClientBlocking,
    url: impl IntoUrl,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Send the request for the file
    let response = client
        .get(url)
        .send()
        .and_then(Response::error_for_status)
        .map_err(ExtractError::ReqwestError)?;

    // The `response` is used to stream in the package data
    crate::read::extract_tar_bz2(response, destination)
}

/// Extracts the contents a `.conda` package archive from the specified remote location.
///
/// ```rust,no_run
/// # use std::path::Path;
/// use rattler_package_streaming::reqwest::extract_conda;
/// use rattler_networking::AuthenticatedClientBlocking;
/// # use reqwest::blocking::Client;
/// let _ = extract_conda(
///     AuthenticatedClientBlocking::default(),
///     "https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda",
///     Path::new("/tmp"))
///     .unwrap();
/// ```
pub fn extract_conda(
    client: AuthenticatedClientBlocking,
    url: impl IntoUrl,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Send the request for the file
    let response = client
        .get(url)
        .send()
        .and_then(Response::error_for_status)
        .map_err(ExtractError::ReqwestError)?;

    // The `response` is used to stream in the package data
    crate::read::extract_conda(response, destination)
}

/// Extracts the contents a package archive from the specified remote location. The type of package
/// is determined based on the path of the url.
///
/// ```rust,no_run
/// # use std::path::Path;
/// use rattler_package_streaming::reqwest::extract;
/// use rattler_networking::AuthenticatedClientBlocking;
/// # use reqwest::blocking::Client;
/// let _ = extract(
///     AuthenticatedClientBlocking::default(),
///     "https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda",
///     Path::new("/tmp"))
///     .unwrap();
/// ```
pub fn extract(
    client: AuthenticatedClientBlocking,
    url: impl IntoUrl,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    let url = url
        .into_url()
        .map_err(reqwest::Error::from)
        .map_err(ExtractError::ReqwestError)?;

    match ArchiveType::try_from(Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?
    {
        ArchiveType::TarBz2 => extract_tar_bz2(client, url, destination),
        ArchiveType::Conda => extract_conda(client, url, destination),
    }
}
