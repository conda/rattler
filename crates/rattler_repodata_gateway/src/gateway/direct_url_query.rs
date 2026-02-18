use std::{future::IntoFuture, sync::Arc};

use futures::FutureExt;
use rattler_cache::package_cache::{CacheKey, PackageCache, PackageCacheError};
use rattler_conda_types::package::{ArchiveIdentifier, CondaArchiveType};
use rattler_conda_types::{
    ConvertSubdirError, PackageRecord, RepoDataRecord,
    package::{CondaArchiveIdentifier, DistArchiveIdentifier, IndexJson, PackageFile},
};
use rattler_digest::{Md5Hash, Sha256Hash};
use rattler_networking::LazyClient;
use rattler_package_streaming::ExtractError;
use url::Url;

pub(crate) struct DirectUrlQuery {
    /// The url to query
    url: Url,
    /// Optional Sha256 of the file
    sha256: Option<Sha256Hash>,
    /// Optional MD5 of the file
    md5: Option<Md5Hash>,
    /// The client to use for fetching the package
    client: LazyClient,
    /// The cache to use for storing the package
    package_cache: PackageCache,
}

#[derive(Debug, thiserror::Error)]
pub enum DirectUrlQueryError {
    #[error(transparent)]
    PackageCache(#[from] PackageCacheError),
    #[error(transparent)]
    IndexJson(#[from] std::io::Error),
    #[error(transparent)]
    ConvertSubdir(#[from] ConvertSubdirError),
    #[error("could not determine archive identifier from url filename '{0}'")]
    InvalidFilename(String),
}

impl DirectUrlQuery {
    pub(crate) fn new(
        url: Url,
        package_cache: PackageCache,
        client: LazyClient,
        sha256: Option<Sha256Hash>,
        md5: Option<Md5Hash>,
    ) -> Self {
        Self {
            url,
            sha256,
            md5,
            client,
            package_cache,
        }
    }

    /// Execute the Repodata query using the cache as a source for the
    /// index.json
    pub async fn execute(self) -> Result<Vec<Arc<RepoDataRecord>>, DirectUrlQueryError> {
        let (index_json, archive_type): (IndexJson, CondaArchiveType) = if let Ok(file_path) =
            self.url.to_file_path()
        {
            // Determine the type of the archive
            let Some(archive_type) = CondaArchiveType::try_from(&file_path) else {
                return Err(DirectUrlQueryError::InvalidFilename(
                    file_path.display().to_string(),
                ));
            };

            match rattler_package_streaming::seek::read_package_file(&file_path) {
                Ok(index_json) => (index_json, archive_type),
                Err(ExtractError::IoError(io)) => return Err(DirectUrlQueryError::IndexJson(io)),
                Err(ExtractError::UnsupportedArchiveType) => {
                    return Err(DirectUrlQueryError::InvalidFilename(
                        file_path.display().to_string(),
                    ));
                }
                Err(e) => {
                    return Err(DirectUrlQueryError::IndexJson(std::io::Error::other(
                        e.to_string(),
                    )));
                }
            }
        } else {
            // Convert the url to an archive identifier.
            let Some(archive_identifier) = CondaArchiveIdentifier::try_from_url(&self.url) else {
                let filename = self.url.path_segments().and_then(Iterator::last);
                return Err(DirectUrlQueryError::InvalidFilename(
                    filename.unwrap_or("").to_string(),
                ));
            };

            // Construct a cache key
            let archive_type = archive_identifier.archive_type;
            let cache_key = CacheKey::from(archive_identifier)
                .with_opt_sha256(self.sha256)
                .with_opt_md5(self.md5);

            // TODO: Optimize this by only parsing the index json from stream.
            // Get package on system
            let cache_lock = self
                .package_cache
                .get_or_fetch_from_url(
                    cache_key,
                    self.url.clone(),
                    self.client.clone(),
                    // Should we add a reporter?
                    None,
                )
                .await?;

            // Extract package record from index json
            (
                IndexJson::from_package_directory(cache_lock.path())?,
                archive_type,
            )
        };

        let identifier = DistArchiveIdentifier::try_from_url(&self.url).unwrap_or_else(|| {
            DistArchiveIdentifier {
                identifier: ArchiveIdentifier {
                    name: index_json.name.as_source().to_string(),
                    version: index_json.version.to_string(),
                    build_string: index_json.build.clone(),
                },
                archive_type: archive_type.into(),
            }
        });

        let package_record = PackageRecord::from_index_json(
            index_json,
            None, // size is unknown for direct urls
            self.sha256,
            self.md5,
        )?;

        tracing::debug!("Package record build from direct url: {:?}", package_record);

        Ok(vec![Arc::new(RepoDataRecord {
            package_record,
            identifier,
            url: self.url.clone(),
            channel: None,
        })])
    }
}

impl IntoFuture for DirectUrlQuery {
    type Output = Result<Vec<Arc<RepoDataRecord>>, DirectUrlQueryError>;

    type IntoFuture = futures::future::BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        self.execute().boxed()
    }
}

#[cfg(test)]
mod test {
    use std::{env::temp_dir, path::PathBuf};

    use rattler_cache::package_cache::PackageCache;
    use url::Url;

    use super::*;

    #[tokio::test]
    async fn test_direct_url_query() {
        let url = Url::parse(
            "https://conda.anaconda.org/conda-forge/noarch/boltons-24.0.0-pyhd8ed1ab_0.conda",
        )
        .unwrap();
        let package_cache = PackageCache::new(PathBuf::from("/tmp"));
        let query = DirectUrlQuery::new(
            url.clone(),
            package_cache,
            LazyClient::default(),
            None,
            None,
        );

        assert_eq!(query.url.clone(), url);

        let repodata_record = query.await.unwrap();

        assert_eq!(
            repodata_record
                .first()
                .unwrap()
                .package_record
                .name
                .as_normalized(),
            "boltons"
        );
        assert_eq!(
            repodata_record
                .first()
                .unwrap()
                .package_record
                .version
                .as_str(),
            "24.0.0"
        );
    }

    #[tokio::test]
    async fn test_direct_url_path_query() {
        let package_path = tools::download_and_cache_file_async(
            "https://conda.anaconda.org/conda-forge/win-64/zlib-1.2.8-vc10_0.tar.bz2"
                .parse()
                .unwrap(),
            "ee9172dbe9ebd158e8e68d6d0f7dc2060f0c8230b44d2e9a3595b7cd7336b915",
        )
        .await
        .unwrap();

        let url = Url::from_file_path(package_path).unwrap();
        let package_cache = PackageCache::new(temp_dir());
        let query = DirectUrlQuery::new(
            url.clone(),
            package_cache,
            LazyClient::default(),
            None,
            None,
        );

        assert_eq!(query.url.clone(), url);

        let repodata_record = query.await.unwrap();
        assert_eq!(
            repodata_record
                .first()
                .unwrap()
                .package_record
                .name
                .as_normalized(),
            "zlib"
        );
        assert_eq!(
            repodata_record
                .first()
                .unwrap()
                .package_record
                .version
                .as_str(),
            "1.2.8"
        );
    }

    #[tokio::test]
    async fn test_direct_url_local_file_with_invalid_filename() {
        // Copy a test package and rename it to something that isn't a valid ArchiveIdentifier
        let temp_dir_path = tempfile::tempdir().unwrap();

        let original_package = tools::project_root()
            .join("test-data")
            .join("packages")
            .join("empty-0.1.0-h4616a5c_0.conda");

        // Rename to a filename that is NOT a valid ArchiveIdentifier
        let renamed_package = temp_dir_path.path().join("my-renamed-package.conda");
        std::fs::copy(&original_package, &renamed_package).unwrap();

        let url = Url::from_file_path(&renamed_package).unwrap();
        let package_cache = PackageCache::new(temp_dir());
        let query = DirectUrlQuery::new(
            url.clone(),
            package_cache,
            LazyClient::default(),
            None,
            None,
        );

        let repodata_record = query.await.unwrap();
        assert_eq!(
            repodata_record
                .first()
                .unwrap()
                .package_record
                .name
                .as_normalized(),
            "empty"
        );
        assert_eq!(
            repodata_record
                .first()
                .unwrap()
                .package_record
                .version
                .as_str(),
            "0.1.0"
        );
    }

    #[tokio::test]
    async fn test_direct_url_local_file_tar_bz2() {
        // Copy a test package and rename it to something that isn't a valid ArchiveIdentifier
        let temp_dir_path = tempfile::tempdir().unwrap();

        let original_package = tools::project_root()
            .join("test-data")
            .join("test-server")
            .join("repo")
            .join("noarch")
            .join("test-package-0.1-0.tar.bz2");

        // Rename to a filename that is NOT a valid ArchiveIdentifier
        let renamed_package = temp_dir_path.path().join("another-package.tar.bz2");
        std::fs::copy(&original_package, &renamed_package).unwrap();

        let url = Url::from_file_path(&renamed_package).unwrap();
        let package_cache = PackageCache::new(temp_dir());
        let query = DirectUrlQuery::new(
            url.clone(),
            package_cache,
            LazyClient::default(),
            None,
            None,
        );

        let repodata_record = query.await.unwrap();
        assert_eq!(
            repodata_record
                .first()
                .unwrap()
                .package_record
                .name
                .as_normalized(),
            "test-package"
        );
        assert_eq!(
            repodata_record
                .first()
                .unwrap()
                .package_record
                .version
                .as_str(),
            "0.1"
        );
    }
}
