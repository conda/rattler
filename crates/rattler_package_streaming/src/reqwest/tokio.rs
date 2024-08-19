//! Functionality to stream and extract packages directly from a [`reqwest::Url`] within a [`tokio`]
//! async context.

use crate::{DownloadReporter, ExtractError, ExtractResult};
use futures_util::stream::TryStreamExt;
use rattler_conda_types::package::ArchiveType;
use rattler_digest::Sha256Hash;
use reqwest::Response;
use std::path::Path;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio_util::either::Either;
use tokio_util::io::StreamReader;
use tracing;
use url::Url;
use zip::result::ZipError;

/// zipfiles may use data descriptors to signal that the decompressor needs to seek ahead in the buffer
/// to find the compressed data length.
/// Since we stream the package over a non seekable HTTP connection, this condition will cause an error during
/// decompression. In this case, we fallback to reading the whole data to a buffer before attempting decompression.
/// Read more in <https://github.com/conda/rattler/issues/794>
const DATA_DESCRIPTOR_ERROR_MESSAGE: &str = "The file length is not available in the local header";

fn error_for_status(response: reqwest::Response) -> reqwest_middleware::Result<Response> {
    response
        .error_for_status()
        .map_err(reqwest_middleware::Error::Reqwest)
}

async fn get_reader(
    url: Url,
    client: reqwest_middleware::ClientWithMiddleware,
    expected_sha256: Option<Sha256Hash>,
    reporter: Option<Arc<dyn DownloadReporter>>,
) -> Result<impl tokio::io::AsyncRead, ExtractError> {
    if let Some(reporter) = &reporter {
        reporter.on_download_start();
    }

    if url.scheme() == "file" {
        let file =
            tokio::fs::File::open(url.to_file_path().expect("Could not convert to file path"))
                .await
                .map_err(ExtractError::IoError)?;

        Ok(Either::Left(BufReader::new(file)))
    } else {
        // Send the request for the file
        let mut request = client.get(url.clone());

        if let Some(sha256) = expected_sha256 {
            // This is used by the OCI registry middleware to verify the sha256 of the response
            request = request.header("X-Expected-Sha256", format!("{sha256:x}"));
        }

        let response = request
            .send()
            .await
            .and_then(error_for_status)
            .map_err(ExtractError::ReqwestError)?;

        let total_bytes = response.content_length();
        let mut bytes_received = Box::new(0);
        let byte_stream = response.bytes_stream().inspect_ok(move |frame| {
            *bytes_received += frame.len() as u64;
            if let Some(reporter) = &reporter {
                reporter.on_download_progress(*bytes_received, total_bytes);
            }
        });

        // Get the response as a stream
        Ok(Either::Right(StreamReader::new(byte_stream.map_err(
            |err| std::io::Error::new(std::io::ErrorKind::Other, err),
        ))))
    }
}

/// Extracts the contents a `.tar.bz2` package archive from the specified remote location.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use url::Url;
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// use rattler_package_streaming::reqwest::tokio::extract_tar_bz2;
/// let _ = extract_tar_bz2(
///     ClientWithMiddleware::from(Client::new()),
///     Url::parse("https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2").unwrap(),
///     Path::new("/tmp"),
///     None,
///     None)
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_tar_bz2(
    client: reqwest_middleware::ClientWithMiddleware,
    url: Url,
    destination: &Path,
    expected_sha256: Option<Sha256Hash>,
    reporter: Option<Arc<dyn DownloadReporter>>,
) -> Result<ExtractResult, ExtractError> {
    let reader = get_reader(url.clone(), client, expected_sha256, reporter.clone()).await?;
    // The `response` is used to stream in the package data
    let result = crate::tokio::async_read::extract_tar_bz2(reader, destination).await?;
    if let Some(reporter) = &reporter {
        reporter.on_download_complete();
    }
    Ok(result)
}

/// Extracts the contents a `.conda` package archive from the specified remote location.
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// # use std::path::Path;
/// use rattler_package_streaming::reqwest::tokio::extract_conda;
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// use url::Url;
/// let _ = extract_conda(
///     ClientWithMiddleware::from(Client::new()),
///     Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap(),
///     Path::new("/tmp"),
///     None,
///     None)
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract_conda(
    client: reqwest_middleware::ClientWithMiddleware,
    url: Url,
    destination: &Path,
    expected_sha256: Option<Sha256Hash>,
    reporter: Option<Arc<dyn DownloadReporter>>,
) -> Result<ExtractResult, ExtractError> {
    // The `response` is used to stream in the package data
    let reader = get_reader(
        url.clone(),
        client.clone(),
        expected_sha256,
        reporter.clone(),
    )
    .await?;
    match crate::tokio::async_read::extract_conda(reader, destination).await {
        Ok(result) => {
            if let Some(reporter) = &reporter {
                reporter.on_download_complete();
            }
            Ok(result)
        }
        // https://github.com/conda/rattler/issues/794
        Err(ExtractError::ZipError(ZipError::UnsupportedArchive(zip_error)))
            if (zip_error.contains(DATA_DESCRIPTOR_ERROR_MESSAGE)) =>
        {
            tracing::warn!("Failed to stream decompress conda package from '{}' due to the presence of zip data descriptors. Falling back to non streaming decompression", url);
            let new_reader =
                get_reader(url.clone(), client, expected_sha256, reporter.clone()).await?;
            crate::tokio::async_read::extract_conda_via_buffering(new_reader, destination).await
        }
        Err(e) => Err(e),
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
/// use reqwest::Client;
/// use reqwest_middleware::ClientWithMiddleware;
/// let _ = extract(
///     ClientWithMiddleware::from(Client::new()),
///     Url::parse("https://conda.anaconda.org/conda-forge/linux-64/python-3.10.8-h4a9ceb5_0_cpython.conda").unwrap(),
///     Path::new("/tmp"),
///     None,
///     None)
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn extract(
    client: reqwest_middleware::ClientWithMiddleware,
    url: Url,
    destination: &Path,
    expected_sha256: Option<Sha256Hash>,
    reporter: Option<Arc<dyn DownloadReporter>>,
) -> Result<ExtractResult, ExtractError> {
    match ArchiveType::try_from(Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?
    {
        ArchiveType::TarBz2 => {
            extract_tar_bz2(client, url, destination, expected_sha256, reporter).await
        }
        ArchiveType::Conda => {
            extract_conda(client, url, destination, expected_sha256, reporter).await
        }
    }
}
