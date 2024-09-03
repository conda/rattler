use std::{future::IntoFuture, sync::Arc};

use futures::FutureExt;
use rattler_cache::package_cache::CacheKey;
use rattler_cache::package_cache::{PackageCache, PackageCacheError};
use rattler_conda_types::{
    package::{ArchiveIdentifier, IndexJson, PackageFile},
    ConvertSubdirError, PackageRecord, RepoDataRecord,
};
use rattler_digest::Sha256Hash;
use url::Url;

pub(crate) struct DirectUrlQuery {
    /// The url to query
    url: Url,
    /// Optional Sha256 of the file
    sha256: Option<Sha256Hash>,
    /// The client to use for fetching the package
    client: reqwest_middleware::ClientWithMiddleware,
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
        client: reqwest_middleware::ClientWithMiddleware,
        sha256: Option<Sha256Hash>,
    ) -> Self {
        Self {
            url,
            sha256,
            client,
            package_cache,
        }
    }

    /// Execute the Repodata query using the cache as a source for the
    /// index.json
    pub async fn execute(self) -> Result<Arc<[RepoDataRecord]>, DirectUrlQueryError> {
        // Convert the url to an archive identifier.
        let Some(archive_identifier) = ArchiveIdentifier::try_from_url(&self.url) else {
            let filename = self.url.path_segments().and_then(Iterator::last);
            return Err(DirectUrlQueryError::InvalidFilename(
                filename.unwrap_or("").to_string(),
            ));
        };

        // Construct a cache key
        let cache_key = CacheKey::from(archive_identifier).with_opt_sha256(self.sha256);

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
        let index_json = IndexJson::from_package_directory(cache_lock.path())?;
        let package_record = PackageRecord::from_index_json(
            index_json,
            None,        // Size
            self.sha256, // sha256
            None,        // md5
        )?;

        tracing::debug!("Package record build from direct url: {:?}", package_record);

        Ok(Arc::new([RepoDataRecord {
            package_record,
            // File name is the same as the url.
            file_name: self.url.clone().to_string(),
            url: self.url.clone(),
            // Fake channel as it is unused in this case.
            channel: "".to_string(),
        }]))
    }
}

impl IntoFuture for DirectUrlQuery {
    type Output = Result<Arc<[RepoDataRecord]>, DirectUrlQueryError>;

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
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        let query = DirectUrlQuery::new(url.clone(), package_cache, client, None);

        assert_eq!(query.url.clone(), url);

        let repodata_record = query.await.unwrap();

        assert_eq!(
            repodata_record
                .as_ref()
                .first()
                .unwrap()
                .package_record
                .name
                .as_normalized(),
            "boltons"
        );
        assert_eq!(
            repodata_record
                .as_ref()
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
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        let query = DirectUrlQuery::new(url.clone(), package_cache, client, None);

        assert_eq!(query.url.clone(), url);

        let repodata_record = query.await.unwrap();
        assert_eq!(
            repodata_record
                .as_ref()
                .first()
                .unwrap()
                .package_record
                .name
                .as_normalized(),
            "zlib"
        );
        assert_eq!(
            repodata_record
                .as_ref()
                .first()
                .unwrap()
                .package_record
                .version
                .as_str(),
            "1.2.8"
        );
    }
}
