use std::future::Future;

use bytes::Bytes;
use futures::{Stream, TryStreamExt};
use url::Url;

use crate::utils::BodyStreamExt;

/// A trait that enables being notified of download progress.
pub trait DownloadReporter: Send + Sync {
    /// Called when a download of a file started.
    ///
    /// Returns an index that can be used to identify the download in subsequent
    /// calls.
    fn on_download_start(&self, _url: &Url) -> usize {
        0
    }

    /// Called when the download of a file makes any progress.
    ///
    /// The `total_bytes` parameter is `None` if the total size of the file is
    /// unknown.
    ///
    /// The `index` parameter is the index returned by `on_download_start`.
    fn on_download_progress(
        &self,
        _url: &Url,
        _index: usize,
        _bytes_downloaded: usize,
        _total_bytes: Option<usize>,
    ) {
    }

    /// Called when the download of a file finished.
    ///
    /// The `index` parameter is the index returned by `on_download_start`.
    fn on_download_complete(&self, _url: &Url, _index: usize) {}
}

/// A trait that enables being notified of repodata fetching progress.
pub trait Reporter: Send + Sync {
    /// Returns a reporter for downloading files.
    fn download_reporter(&self) -> Option<&dyn DownloadReporter>;
}

#[allow(dead_code)]
pub(crate) trait ResponseReporterExt {
    /// Converts a response into a stream of bytes, notifying a reporter of the
    /// progress.
    fn byte_stream_with_progress(
        self,
        reporter: Option<(&dyn DownloadReporter, usize)>,
    ) -> impl Stream<Item = reqwest::Result<Bytes>>;

    /// Reads all the bytes from a stream and notifies a reporter of the
    /// progress.
    fn bytes_with_progress(
        self,
        reporter: Option<(&dyn DownloadReporter, usize)>,
    ) -> impl Future<Output = reqwest::Result<Vec<u8>>>;

    /// Reads all the bytes from a stream and convert it to text and notifies a
    /// reporter of the progress.
    fn text_with_progress(
        self,
        reporter: Option<(&dyn DownloadReporter, usize)>,
    ) -> impl Future<Output = reqwest::Result<String>>;
}

impl ResponseReporterExt for reqwest::Response {
    fn byte_stream_with_progress(
        self,
        reporter: Option<(&dyn DownloadReporter, usize)>,
    ) -> impl Stream<Item = reqwest::Result<Bytes>> {
        let total_size = self.content_length().map(|len| len as usize);
        let url = self.url().clone();
        let mut bytes_read = 0;
        self.bytes_stream().inspect_ok(move |bytes| {
            if let Some((reporter, index)) = reporter {
                bytes_read += bytes.len();
                reporter.on_download_progress(&url, index, bytes_read, total_size);
            }
        })
    }

    fn bytes_with_progress(
        self,
        reporter: Option<(&dyn DownloadReporter, usize)>,
    ) -> impl Future<Output = reqwest::Result<Vec<u8>>> {
        self.byte_stream_with_progress(reporter).bytes()
    }

    fn text_with_progress(
        self,
        reporter: Option<(&dyn DownloadReporter, usize)>,
    ) -> impl Future<Output = reqwest::Result<String>> {
        self.byte_stream_with_progress(reporter).text()
    }
}
