//! A cache of package archives (`.conda` and `.tar.bz2` files) as they were downloaded.
//!
//! The [`crate::package_cache::PackageCache`] stores packages extracted on the local filesystem,
//! which is what installing into a local prefix needs. Some consumers instead read the files of a
//! package directly from its archive, for example to build a container image for another
//! platform without extracting the package. They can use this cache to avoid downloading the
//! archives over and over.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use futures::StreamExt;
use rattler_conda_types::{RepoDataRecord, package::CondaArchiveIdentifier};
use rattler_digest::{Md5, Md5Hash, Sha256, Sha256Hash};
use rattler_package_streaming::DownloadReporter;
use rattler_redaction::Redact;
use url::Url;

/// An error that can occur when fetching an archive into the [`ArchiveCache`].
#[derive(Debug, thiserror::Error)]
pub enum ArchiveCacheError {
    /// The url does not point to a conda package archive.
    #[error("'{0}' does not point to a conda package archive")]
    NotAnArchive(String),

    /// The archive could not be downloaded.
    #[error("failed to download '{0}'")]
    FailedToDownload(String, #[source] Box<reqwest_middleware::Error>),

    /// An IO error occurred.
    #[error("{0}")]
    Io(String, #[source] std::io::Error),

    /// The hash of the downloaded archive does not match the expected hash.
    #[error("hash mismatch for '{url}': expected {expected}, got {actual}")]
    HashMismatch {
        /// The url of the archive.
        url: String,
        /// The expected hash.
        expected: String,
        /// The hash of the downloaded archive.
        actual: String,
    },
}

fn redacted(url: &Url) -> String {
    url.clone().redact().to_string()
}

/// The hash a downloaded archive is validated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveHash {
    /// The SHA256 hash of the archive.
    Sha256(Sha256Hash),
    /// The MD5 hash of the archive.
    Md5(Md5Hash),
}

impl ArchiveHash {
    /// The strongest hash that is available for `record`.
    pub fn from_record(record: &RepoDataRecord) -> Option<Self> {
        let package_record = &record.package_record;
        package_record
            .sha256
            .map(Self::Sha256)
            .or(package_record.md5.map(Self::Md5))
    }

    fn to_hex(self) -> String {
        match self {
            Self::Sha256(hash) => hex::encode(hash),
            Self::Md5(hash) => hex::encode(hash),
        }
    }
}

/// A cache of downloaded package archives, see the [module documentation](self).
///
/// Archives are stored as `<name>-<version>-<build>-<hash>.<extension>` where `<hash>` is the
/// expected hash of the archive (or a hash of its url if no hash is known). Archives are only
/// moved into the cache after they have been downloaded completely and validated, so an archive in
/// the cache is always complete.
#[derive(Debug, Clone)]
pub struct ArchiveCache {
    path: PathBuf,
}

impl ArchiveCache {
    /// Constructs a new cache that stores the archives in `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The directory that contains the archives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the location of the archive of `record`, downloading it if it is not in the cache
    /// yet. Archives with a `file://` url are not copied into the cache.
    pub async fn get_or_fetch(
        &self,
        record: &RepoDataRecord,
        client: &reqwest_middleware::ClientWithMiddleware,
        reporter: Option<Arc<dyn DownloadReporter>>,
    ) -> Result<PathBuf, ArchiveCacheError> {
        self.get_or_fetch_from_url(
            &record.url,
            ArchiveHash::from_record(record),
            client,
            reporter,
        )
        .await
    }

    /// Returns the location of the archive at `url`, downloading it if it is not in the cache
    /// yet. The download is validated against `hash` if one is given. Archives with a `file://`
    /// url are not copied into the cache.
    pub async fn get_or_fetch_from_url(
        &self,
        url: &Url,
        hash: Option<ArchiveHash>,
        client: &reqwest_middleware::ClientWithMiddleware,
        reporter: Option<Arc<dyn DownloadReporter>>,
    ) -> Result<PathBuf, ArchiveCacheError> {
        if url.scheme() == "file"
            && let Ok(path) = url.to_file_path()
        {
            return Ok(path);
        }

        let identifier = CondaArchiveIdentifier::try_from_url(url)
            .ok_or_else(|| ArchiveCacheError::NotAnArchive(redacted(url)))?;
        let key = if let Some(hash) = hash {
            hash.to_hex()
        } else {
            let url_hash = rattler_digest::compute_bytes_digest::<Sha256>(url.as_str());
            format!("url-{}", &hex::encode(url_hash)[..16])
        };
        let destination = self.path.join(format!(
            "{}-{key}{}",
            identifier.identifier,
            identifier.archive_type.extension()
        ));
        if destination.is_file() {
            return Ok(destination);
        }

        self.download(url, hash, &destination, client, reporter)
            .await?;
        Ok(destination)
    }

    async fn download(
        &self,
        url: &Url,
        hash: Option<ArchiveHash>,
        destination: &Path,
        client: &reqwest_middleware::ClientWithMiddleware,
        reporter: Option<Arc<dyn DownloadReporter>>,
    ) -> Result<(), ArchiveCacheError> {
        let io_error = |what: &str| {
            let what = what.to_owned();
            move |e| ArchiveCacheError::Io(what, e)
        };
        crate::ensure_cache_dir(&self.path).map_err(io_error("failed to create the cache"))?;

        let response = client
            .get(url.clone())
            .send()
            .await
            .and_then(|response| response.error_for_status().map_err(Into::into))
            .map_err(|e| ArchiveCacheError::FailedToDownload(redacted(url), Box::new(e)))?;

        if let Some(reporter) = &reporter {
            reporter.on_download_start();
        }
        let total_bytes = response.content_length();

        // Download into a temporary file next to the destination, so it can be moved into place
        // atomically once it is complete.
        let temp_file = tempfile::NamedTempFile::with_prefix_in(".download-", &self.path)
            .map_err(io_error("failed to create a temporary file"))?;
        let mut writer = std::io::BufWriter::new(temp_file);
        let mut sha256 = <Sha256 as digest::Digest>::new();
        let mut md5 = <Md5 as digest::Digest>::new();
        let mut bytes_received = 0;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                ArchiveCacheError::FailedToDownload(redacted(url), Box::new(e.into()))
            })?;
            digest::Digest::update(&mut sha256, &chunk);
            digest::Digest::update(&mut md5, &chunk);
            writer
                .write_all(&chunk)
                .map_err(io_error("failed to write the archive"))?;
            bytes_received += chunk.len() as u64;
            if let Some(reporter) = &reporter {
                reporter.on_download_progress(bytes_received, total_bytes);
            }
        }
        let temp_file = writer.into_inner().map_err(|e| {
            ArchiveCacheError::Io("failed to write the archive".into(), e.into_error())
        })?;

        let actual = match hash {
            Some(ArchiveHash::Sha256(_)) | None => {
                ArchiveHash::Sha256(digest::Digest::finalize(sha256))
            }
            Some(ArchiveHash::Md5(_)) => ArchiveHash::Md5(digest::Digest::finalize(md5)),
        };
        if let Some(expected) = hash
            && expected != actual
        {
            return Err(ArchiveCacheError::HashMismatch {
                url: redacted(url),
                expected: expected.to_hex(),
                actual: actual.to_hex(),
            });
        }

        // Another process may have downloaded the same archive in the meantime, replacing it is
        // fine because both are valid.
        temp_file.persist(destination).map_err(|e| {
            ArchiveCacheError::Io("failed to move the archive into place".into(), e.error)
        })?;

        if let Some(reporter) = &reporter {
            reporter.on_download_complete();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, path::PathBuf};

    use assert_matches::assert_matches;
    use axum::Router;
    use rattler_digest::{Sha256, Sha256Hash, compute_file_digest};
    use tower_http::services::ServeDir;
    use url::Url;

    use super::{ArchiveCache, ArchiveCacheError, ArchiveHash};

    fn test_data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
    }

    async fn serve_test_data() -> SocketAddr {
        let service = Router::new().fallback_service(ServeDir::new(test_data_dir()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
        addr
    }

    #[tokio::test]
    async fn downloads_and_reuses_archives() {
        let addr = serve_test_data().await;
        let archive = "clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2";
        let url = Url::parse(&format!("http://{addr}/{archive}")).unwrap();
        let sha256 = compute_file_digest::<Sha256>(test_data_dir().join(archive)).unwrap();

        let cache_dir = tempfile::tempdir().unwrap();
        let cache = ArchiveCache::new(cache_dir.path());
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let path = cache
            .get_or_fetch_from_url(&url, Some(ArchiveHash::Sha256(sha256)), &client, None)
            .await
            .unwrap();
        assert!(path.starts_with(cache_dir.path()));
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("clobber-1-0.1.0-h4616a5c_0-{}.tar.bz2", hex::encode(sha256))
        );
        assert_eq!(compute_file_digest::<Sha256>(&path).unwrap(), sha256);

        // The second request is served from the cache, even without a server.
        let unreachable = Url::parse(&format!("http://127.0.0.1:1/{archive}")).unwrap();
        let cached = cache
            .get_or_fetch_from_url(
                &unreachable,
                Some(ArchiveHash::Sha256(sha256)),
                &client,
                None,
            )
            .await
            .unwrap();
        assert_eq!(cached, path);
    }

    #[tokio::test]
    async fn rejects_hash_mismatch() {
        let addr = serve_test_data().await;
        let url = Url::parse(&format!(
            "http://{addr}/clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2"
        ))
        .unwrap();

        let cache_dir = tempfile::tempdir().unwrap();
        let cache = ArchiveCache::new(cache_dir.path());
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let result = cache
            .get_or_fetch_from_url(
                &url,
                Some(ArchiveHash::Sha256(Sha256Hash::default())),
                &client,
                None,
            )
            .await;
        assert_matches!(result, Err(ArchiveCacheError::HashMismatch { .. }));
        // Nothing but the cache tag is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(cache_dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "CACHEDIR.TAG")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[tokio::test]
    async fn file_urls_are_not_copied() {
        let path = test_data_dir().join("clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2");
        let url = Url::from_file_path(path.canonicalize().unwrap()).unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let cache = ArchiveCache::new(cache_dir.path());
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        let result = cache
            .get_or_fetch_from_url(&url, None, &client, None)
            .await
            .unwrap();
        assert_eq!(result, path.canonicalize().unwrap());
    }
}
