use std::sync::Arc;

use ::tokio::io::{AsyncSeekExt, AsyncWriteExt};
use fs_err::tokio;
use futures::StreamExt;
use rattler_package_streaming::DownloadReporter;
use tempfile::NamedTempFile;
use url::Url;

/// Download the contents of the archive from the specified remote location
/// and store it in a temporary file.
pub(crate) async fn download(
    client: reqwest_middleware::ClientWithMiddleware,
    url: Url,
    suffix: &str,
    reporter: Option<Arc<dyn DownloadReporter>>,
) -> Result<NamedTempFile, DownloadError> {
    let temp_file = NamedTempFile::with_suffix(suffix)?;

    // Send the request for the file
    let response = client.get(url.clone()).send().await?.error_for_status()?;

    if let Some(reporter) = &reporter {
        reporter.on_download_start();
    }

    let total_bytes = response.content_length();
    let (tmp_file_handle, tmp_path) = temp_file.into_parts();
    // Convert the named temp file into a tokio file
    let mut file = tokio::File::from_std(fs_err::File::from_parts(tmp_file_handle, &tmp_path));

    let mut stream = response.bytes_stream();

    let mut bytes_received = 0;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;

        if let Some(reporter) = &reporter {
            bytes_received += chunk.len() as u64;
            reporter.on_download_progress(bytes_received, total_bytes);
        }
        file.write_all(&chunk).await?;
    }

    file.flush().await?;

    file.rewind().await?;

    let file_handle = file.into_parts().0.into_std().await;

    Ok(NamedTempFile::from_parts(file_handle, tmp_path))
}

/// An error that can occur when downloading an archive.
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum DownloadError {
    #[error("an io error occurred: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    ReqwestMiddleware(#[from] ::reqwest_middleware::Error),

    #[error(transparent)]
    Reqwest(#[from] ::reqwest::Error),
}
