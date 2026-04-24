use std::future::Future;

use bytes::Bytes;
use futures::{Stream, TryStreamExt};
#[cfg(feature = "gateway")]
use rattler_conda_types::Channel;
#[cfg(feature = "sparse")]
use rattler_conda_types::{RepodataRevision, RepodataRevisionInfo};
#[cfg(feature = "gateway")]
use rattler_redaction::Redact;
use url::Url;

use crate::utils::BodyStreamExt;

/// The newest repodata revision understood by this version of rattler.
///
/// Revision `3` is the current experimental top-level `v3` map implemented by
/// rattler. Newer revisions are intentionally ignored by older clients, but we
/// still surface their metadata for user-facing warnings.
#[cfg(feature = "sparse")]
pub const SUPPORTED_REPODATA_REVISION: RepodataRevision = RepodataRevision::V3;

/// A structured message indicating that a channel contains repodata newer than
/// this client understands.
#[cfg(feature = "sparse")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedRepodataRevision {
    /// Redacted channel base URL.
    pub channel: String,

    /// Channel subdirectory, for example `linux-64` or `noarch`.
    pub subdir: String,

    /// The newest revision supported by this client.
    pub supported_revision: RepodataRevision,

    /// Metadata for the unsupported revision advertised by the channel.
    pub revision: RepodataRevisionInfo,
}

#[cfg(feature = "sparse")]
impl std::fmt::Display for UnsupportedRepodataRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} contains repodata revision {}, but this client only supports up to {}",
            self.channel, self.revision.revision, self.supported_revision
        )?;

        if let Some(n_packages) = self.revision.n_packages {
            write!(f, " ({n_packages} packages may be unavailable)")?;
        }

        Ok(())
    }
}

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

    /// Called when a channel advertises a repodata revision newer than this
    /// client supports.
    #[cfg(feature = "sparse")]
    fn on_unsupported_repodata_revision(&self, _message: &UnsupportedRepodataRevision) {}
}

#[cfg(feature = "gateway")]
pub(crate) fn report_unsupported_repodata_revisions<'a>(
    reporter: Option<&dyn Reporter>,
    channel: &Channel,
    subdir: &str,
    revisions: impl IntoIterator<Item = &'a RepodataRevisionInfo>,
) {
    let Some(reporter) = reporter else {
        return;
    };

    let channel = channel.base_url.url().clone().redact().to_string();
    for revision in revisions {
        if revision.revision > SUPPORTED_REPODATA_REVISION {
            reporter.on_unsupported_repodata_revision(&UnsupportedRepodataRevision {
                channel: channel.clone(),
                subdir: subdir.to_string(),
                supported_revision: SUPPORTED_REPODATA_REVISION,
                revision: revision.clone(),
            });
        }
    }
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
