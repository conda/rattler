//! Functionality to stream and extract packages directly from a
//! [`reqwest::Url`] within a [`tokio`] async context.

use std::{
    path::Path,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use fs_err::tokio as tokio_fs;
use futures_util::stream::TryStreamExt;
use rattler_conda_types::package::CondaArchiveType;
use rattler_digest::Sha256Hash;
use reqwest::Response;
use tokio::io::{AsyncRead, BufReader, ReadBuf};
use tokio_util::{either::Either, io::StreamReader};
use tracing;
use url::Url;
use zip::result::ZipError;

use crate::{DownloadReporter, ExtractError, ExtractResult};

/// zip files may use data descriptors to signal that the decompressor needs to
/// seek ahead in the buffer to find the compressed data length.
/// Since we stream the package over a non seek-able HTTP connection, this
/// condition will cause an error during decompression. In this case, we
/// fallback to reading the whole data to a buffer before attempting
/// decompression. Read more in <https://github.com/conda/rattler/issues/794>
const DATA_DESCRIPTOR_ERROR_MESSAGE: &str = "The file length is not available in the local header";

fn error_for_status(response: reqwest::Response) -> reqwest_middleware::Result<Response> {
    response
        .error_for_status()
        .map_err(reqwest_middleware::Error::Reqwest)
}

/// Reports completion when the response body reaches EOF (or is dropped), rather
/// than when extraction of that body completes.
struct ReportingReader<R> {
    inner: R,
    reporter: Option<Arc<dyn DownloadReporter>>,
    bytes_read: u64,
    total_bytes: Option<u64>,
    completed: bool,
}

impl<R> ReportingReader<R> {
    fn complete(&mut self) {
        if !self.completed {
            if let Some(reporter) = &self.reporter {
                reporter.on_download_complete();
            }
            self.completed = true;
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ReportingReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let filled_before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let bytes_read = buf.filled().len() - filled_before;
                if bytes_read == 0 {
                    self.complete();
                } else {
                    self.bytes_read += bytes_read as u64;
                    if let Some(reporter) = &self.reporter {
                        reporter.on_download_progress(self.bytes_read, self.total_bytes);
                    }
                }
                Poll::Ready(Ok(()))
            }
            result => result,
        }
    }
}

impl<R> Drop for ReportingReader<R> {
    fn drop(&mut self) {
        self.complete();
    }
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

    let (reader, total_bytes) = if url.scheme() == "file" {
        let file =
            tokio_fs::File::open(url.to_file_path().expect("Could not convert to file path"))
                .await
                .map_err(ExtractError::IoError)?;
        let total_bytes = file.metadata().await.map_err(ExtractError::IoError)?.len();

        (Either::Left(BufReader::new(file)), Some(total_bytes))
    } else {
        // Send the request for the file
        let mut request = client.get(url.clone());

        if let Some(sha256) = expected_sha256 {
            // This is used by the OCI registry middleware to verify the sha256 of the
            // response
            request = request.header("X-Expected-Sha256", hex::encode(sha256));
        }

        let response = request
            .send()
            .await
            .and_then(error_for_status)
            .map_err(ExtractError::ReqwestError)?;

        let total_bytes = response.content_length();
        let byte_stream = response.bytes_stream();

        // Get the response as a stream
        let reader = Either::Right(StreamReader::new(byte_stream.map_err(|err| {
            if err.is_body() {
                std::io::Error::new(std::io::ErrorKind::Interrupted, err)
            } else if err.is_decode() {
                std::io::Error::new(std::io::ErrorKind::InvalidData, err)
            } else {
                std::io::Error::other(err)
            }
        })));
        (reader, total_bytes)
    };

    Ok(ReportingReader {
        inner: reader,
        reporter,
        bytes_read: 0,
        total_bytes,
        completed: false,
    })
}

/// Extracts the contents a `.tar.bz2` package archive from the specified remote
/// location.
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
    crate::tokio::async_read::extract_tar_bz2(reader, destination).await
}

/// Extracts the contents a `.conda` package archive from the specified remote
/// location.
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
        Ok(result) => Ok(result),
        // https://github.com/conda/rattler/issues/794
        Err(ExtractError::ZipError(ZipError::UnsupportedArchive(zip_error)))
            if (zip_error.contains(DATA_DESCRIPTOR_ERROR_MESSAGE)) =>
        {
            tracing::warn!(
                "Failed to stream decompress conda package from '{}' due to the presence of zip data descriptors. Falling back to non streaming decompression",
                url
            );
            let new_reader = get_reader(url, client, expected_sha256, reporter).await?;
            crate::tokio::async_read::extract_conda_via_buffering(new_reader, destination).await
        }
        Err(e) => Err(e),
    }
}

/// Extracts the contents a package archive from the specified remote location.
/// The type of package is determined based on the path of the url.
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
    match CondaArchiveType::try_from(Path::new(url.path()))
        .ok_or(ExtractError::UnsupportedArchiveType)?
    {
        CondaArchiveType::TarBz2 => {
            extract_tar_bz2(client, url, destination, expected_sha256, reporter).await
        }
        CondaArchiveType::Conda => {
            extract_conda(client, url, destination, expected_sha256, reporter).await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::AsyncReadExt;

    use super::*;

    #[derive(Default)]
    struct TestReporter {
        completed: AtomicUsize,
        progress: std::sync::Mutex<Vec<(u64, Option<u64>)>>,
    }

    impl DownloadReporter for TestReporter {
        fn on_download_start(&self) {}
        fn on_download_progress(&self, bytes_downloaded: u64, total_bytes: Option<u64>) {
            self.progress
                .lock()
                .unwrap()
                .push((bytes_downloaded, total_bytes));
        }
        fn on_download_complete(&self) {
            self.completed.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn reports_bytes_consumed_and_completion_at_eof() {
        let reporter = Arc::new(TestReporter::default());
        let mut reader = ReportingReader {
            inner: tokio::io::AsyncReadExt::take(tokio::io::repeat(0), 10),
            reporter: Some(reporter.clone()),
            bytes_read: 0,
            total_bytes: Some(10),
            completed: false,
        };

        let mut contents = Vec::new();
        reader.read_to_end(&mut contents).await.unwrap();
        assert_eq!(
            reporter.progress.lock().unwrap().last(),
            Some(&(10, Some(10)))
        );
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 1);

        drop(reader);
        assert_eq!(reporter.completed.load(Ordering::Relaxed), 1);
    }
}
