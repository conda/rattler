//! Functionality to build a [`RepoDataRecord`] directly from a local
//! `.conda` or `.tar.bz2` package file, without requiring an index or
//! channel.
//!

use std::path::Path;

use rattler_conda_types::{
    ConvertSubdirError, PackageRecord, RepoDataRecord,
    package::{
        ArchiveIdentifier, CondaArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier,
        IndexJson,
    },
};
use rattler_digest::Sha256;

use crate::{ExtractError, seek::read_package_file};

/// An error that can occur while building a [`RepoDataRecord`] from a local
/// package file.
#[derive(Debug, thiserror::Error)]
pub enum LocalPackageRecordError {
    /// An error occurred while reading the package archive.
    #[error(transparent)]
    Extract(#[from] ExtractError),

    /// An io error occurred while computing the size or hashes of the file.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// The `index.json` could not be converted into a [`PackageRecord`].
    #[error(transparent)]
    Convert(#[from] ConvertSubdirError),

    /// The given path could not be turned into a valid file identifier or
    /// url.
    #[error("`{0}` is not a valid package archive path")]
    InvalidPath(std::path::PathBuf),

    /// The blocking task was cancelled before it could complete, e.g.
    /// because the async runtime is shutting down.
    #[error("the task was cancelled")]
    Cancelled,
}

impl From<simple_spawn_blocking::Cancelled> for LocalPackageRecordError {
    fn from(_value: simple_spawn_blocking::Cancelled) -> Self {
        Self::Cancelled
    }
}

/// Builds a [`RepoDataRecord`] directly from a local `.conda` or `.tar.bz2`
/// package file, without requiring a channel or `repodata.json`.
///
/// The resulting record can be passed straight into
/// `Installer::install`; the installer already fetches `file://` records
/// directly from disk instead of downloading them.
///
/// # Example
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() {
/// use rattler_package_streaming::local_package::repodata_record_from_package_archive;
///
/// let record = repodata_record_from_package_archive("/path/to/local/package.conda")
///     .await
///     .unwrap();
/// # }
/// ```
pub async fn repodata_record_from_package_archive(
    package_path: impl AsRef<Path> + Send + 'static,
) -> Result<RepoDataRecord, LocalPackageRecordError> {
    simple_spawn_blocking::tokio::run_blocking_task(move || {
        let package_path = package_path.as_ref();

        // Determine the archive type from the file extension first: it's
        // cheap (no IO) and lets us fail fast on a non-conda archive path
        // before doing any actual work.
        let archive_type = CondaArchiveType::try_from(package_path)
            .ok_or_else(|| LocalPackageRecordError::InvalidPath(package_path.to_path_buf()))?;

        let index_json: IndexJson = read_package_file(package_path)?;

        // Prefer deriving the identifier straight from the filename, since
        // that is the canonical form. But local files aren't always named
        // following the `name-version-build.ext` convention (e.g. a
        // package downloaded and renamed by a user), so fall back to
        // building the identifier from the archive's own `index.json`
        // metadata instead of failing outright.
        let identifier = CondaArchiveIdentifier::try_from_path(package_path).map_or_else(
            || {
                DistArchiveIdentifier::new(
                    ArchiveIdentifier {
                        name: index_json.name.as_source().to_string(),
                        version: index_json.version.to_string(),
                        build_string: index_json.build.clone(),
                    },
                    archive_type,
                )
            },
            Into::into,
        );

        let size = std::fs::metadata(package_path)?.len();
        let sha256 = rattler_digest::compute_file_digest::<Sha256>(package_path)?;

        let package_record =
            PackageRecord::from_index_json(index_json, Some(size), Some(sha256), None)?;

        // `Url::from_file_path` requires an absolute path, so canonicalize
        // relative paths first (the file must exist at this point since we
        // already read its `index.json` above).
        let absolute_path = std::fs::canonicalize(package_path)?;
        let url = url::Url::from_file_path(&absolute_path)
            .map_err(|()| LocalPackageRecordError::InvalidPath(package_path.to_path_buf()))?;

        Ok(RepoDataRecord {
            package_record,
            identifier,
            url,
            channel: None,
        })
    })
    .await
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn test_repodata_record_from_package_archive_conda() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-fd-1-0.1.0-h4616a5c_0.conda");

        let record = repodata_record_from_package_archive(path.clone())
            .await
            .unwrap();

        assert_eq!(record.package_record.name.as_normalized(), "clobber-fd-1");
        assert_eq!(record.channel, None);
        assert_eq!(
            record.url,
            url::Url::from_file_path(std::fs::canonicalize(&path).unwrap()).unwrap()
        );
        assert!(record.package_record.sha256.is_some());
        assert!(record.package_record.md5.is_none());
        assert!(record.package_record.size.is_some());
    }

    #[tokio::test]
    async fn test_repodata_record_from_package_archive_tar_bz2() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2");

        let record = repodata_record_from_package_archive(path.clone())
            .await
            .unwrap();

        assert_eq!(record.package_record.name.as_normalized(), "clobber-1");
        assert_eq!(record.channel, None);
        assert_eq!(
            record.url,
            url::Url::from_file_path(std::fs::canonicalize(&path).unwrap()).unwrap()
        );
    }

    #[tokio::test]
    async fn test_repodata_record_from_package_archive_with_renamed_filename() {
        // A local package file doesn't necessarily follow the
        // `name-version-build.ext` filename convention (e.g. it may have
        // been renamed after being downloaded). The identifier should still
        // be derived from the archive's own `index.json` metadata in that
        // case, instead of failing outright.
        let original_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber/clobber-fd-1-0.1.0-h4616a5c_0.conda");

        let temp_dir = tempfile::tempdir().unwrap();
        let renamed_path = temp_dir.path().join("my-renamed-package.conda");
        std::fs::copy(&original_path, &renamed_path).unwrap();

        let record = repodata_record_from_package_archive(renamed_path)
            .await
            .unwrap();

        assert_eq!(record.package_record.name.as_normalized(), "clobber-fd-1");
        assert_eq!(record.channel, None);
    }
}
