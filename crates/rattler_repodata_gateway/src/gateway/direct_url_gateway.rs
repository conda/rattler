use std::future::IntoFuture;

use futures::FutureExt;
use rattler_cache::package_cache::{PackageCache, PackageCacheError};
use rattler_conda_types::{
    package::{IndexJson, PackageFile},
    ConvertSubdirError, PackageRecord, RepoDataRecord,
};
use url::Url;

pub(crate) struct DirectUrlQuery {
    /// The url to query
    url: Url,
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
}

impl DirectUrlQuery {
    pub(crate) fn new(
        url: Url,
        package_cache: PackageCache,
        client: reqwest_middleware::ClientWithMiddleware,
    ) -> Self {
        Self {
            url,
            client,
            package_cache,
        }
    }

    /// Execute the Repodata query using the cache as a source for the
    /// index.json
    pub async fn execute(self) -> Result<RepoDataRecord, DirectUrlQueryError> {
        // TODO: Optimize this by only parsing the index json from stream.
        // Get package on system
        let package_dir = self
            .package_cache
            .get_or_fetch_from_url(
                // Using the url as cache key
                &self.url,
                self.url.clone(),
                self.client.clone(),
                // Should we add a reporter?
                None,
            )
            .await?;

        // Extract package record from index json
        let index_json = IndexJson::from_package_directory(package_dir)?;
        let package_record = PackageRecord::from_index_json(
            index_json, // Size
            None,       // sha256
            None,       // md5
            None,
        )?
        .with_package_url(self.url.clone());

        tracing::debug!("Package record build from direct url: {:?}", package_record);

        Ok(RepoDataRecord {
            package_record,
            // File name is the same as the url.
            file_name: self.url.clone().to_string(),
            url: self.url.clone(),
            // Fake channel as it is unused in this case.
            channel: "virtual_direct_url_channel".to_string(),
        })
    }
}

impl IntoFuture for DirectUrlQuery {
    type Output = Result<RepoDataRecord, DirectUrlQueryError>;

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
        let query = DirectUrlQuery::new(url.clone(), package_cache, client);

        assert_eq!(query.url.clone(), url);

        let repodata_record = query.await.unwrap();

        assert_eq!(
            repodata_record.package_record.name.as_normalized(),
            "boltons"
        );
        assert_eq!(repodata_record.package_record.version.as_str(), "24.0.0");
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

        let path = temp_dir().join("not_a_conda_archive_style_name.tar.bz2");

        // copy path into fake filename into tmp
        std::fs::copy(package_path, &path).unwrap();

        let url = Url::from_file_path(path).unwrap();
        let package_cache = PackageCache::new(temp_dir());
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        let query = DirectUrlQuery::new(url.clone(), package_cache, client);

        assert_eq!(query.url.clone(), url);

        let repodata_record = query.await.unwrap();
        assert_eq!(repodata_record.package_record.name.as_normalized(), "zlib");
        assert_eq!(repodata_record.package_record.version.as_str(), "1.2.8");
    }
}
