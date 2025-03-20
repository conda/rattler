use crate::index::file_store::FileStore;

use crate::index::html::{parse_package_names_html, parse_project_info_html};
use crate::index::http::{CacheMode, Http, HttpRequestError};
use crate::index::package_sources::PackageSources;
use crate::types::{ArtifactInfo, ProjectInfo, PypiVersion, WheelCoreMetadata};

use crate::{types::InnerAsArtifactName, types::NormalizedPackageName, types::WheelFilename};
use async_http_range_reader::{AsyncHttpRangeReader, CheckSupportMethod};
use elsa::sync::FrozenMap;
use futures::{pin_mut, stream, StreamExt};
use indexmap::IndexMap;
use miette::{self, Diagnostic, IntoDiagnostic};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Method;

use reqwest::{header::CACHE_CONTROL, StatusCode};
use reqwest_middleware::ClientWithMiddleware;
use std::borrow::Borrow;

use std::path::PathBuf;

use itertools::Itertools;
use std::sync::Arc;
use std::{fmt::Display, io::Read, path::Path};

use crate::index::lazy_metadata::lazy_read_wheel_metadata;
use url::Url;

type VersionArtifacts = IndexMap<PypiVersion, Vec<Arc<ArtifactInfo>>>;

/// Cache of the available packages, artifacts and their metadata.
pub struct PackageDb {
    http: Http,

    sources: PackageSources,

    /// A file store that stores metadata by hashes
    metadata_cache: FileStore,

    /// A cache of package name to version to artifacts.
    artifacts: FrozenMap<NormalizedPackageName, Box<VersionArtifacts>>,

    /// Reference to the cache directory for all caches
    cache_dir: PathBuf,

    /// Option to that determines if we always want to check if there are new available artifacts
    check_available_artifacts: CheckAvailablePackages,
}

/// Type of request to get from the `available_artifacts` function.
pub enum ArtifactRequest {
    /// Get the available artifacts from the index.
    FromIndex(NormalizedPackageName),
}

/// Specifies if we always want to check if there are new available artifacts
/// for a package or if we use the time that the server says the request is fresh
#[derive(Default, Eq, PartialEq, Copy, Clone)]
pub enum CheckAvailablePackages {
    /// Always check if there are new available artifacts
    #[default]
    Always,
    /// Trust the time that the server says the request is fresh
    UseServerTime,
}

impl PackageDb {
    /// Constructs a new [`PackageDb`] that reads information from the specified URLs.
    pub fn new(
        package_sources: PackageSources,
        client: ClientWithMiddleware,
        cache_dir: &Path,
        check_available_artifacts: CheckAvailablePackages,
    ) -> miette::Result<Self> {
        let http = Http::new(
            client,
            FileStore::new(&cache_dir.join("http")).into_diagnostic()?,
        );

        let metadata_cache = FileStore::new(&cache_dir.join("metadata")).into_diagnostic()?;

        Ok(Self {
            http,
            sources: package_sources,
            metadata_cache,
            artifacts: FrozenMap::default(),
            cache_dir: cache_dir.to_owned(),
            check_available_artifacts,
        })
    }

    /// Returns the cache directory
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Downloads and caches information about available artifacts of a package from the index.
    ///
    /// TOO: This probably doesn't make sense to keep as an enum/match statement when only a single
    ///      option is defined.
    pub async fn available_artifacts(
        &self,
        request: ArtifactRequest,
    ) -> miette::Result<&IndexMap<PypiVersion, Vec<Arc<ArtifactInfo>>>> {
        match request {
            ArtifactRequest::FromIndex(p) => {
                if let Some(cached) = self.artifacts.get(&p) {
                    return Ok(cached);
                }
                // Start downloading the information for each url.
                let http = self.http.clone();
                let index_urls = self.sources.index_url(&p);

                let urls = index_urls
                    .into_iter()
                    .map(|url| url.join(&format!("{}/", p.as_str())).expect("invalid url"))
                    .collect_vec();
                let request_iter = stream::iter(urls)
                    .map(|url| fetch_simple_api(&http, url, self.check_available_artifacts))
                    .buffer_unordered(10)
                    .filter_map(|result| async { result.transpose() });

                pin_mut!(request_iter);

                // Add all the incoming results to the set of results
                let mut result = VersionArtifacts::default();
                while let Some(response) = request_iter.next().await {
                    for artifact in response?.files {
                        result
                            .entry(PypiVersion::Version {
                                version: artifact.filename.version().clone(),
                                package_allows_prerelease: artifact
                                    .filename
                                    .version()
                                    .any_prerelease(),
                            })
                            .or_default()
                            .push(Arc::new(artifact));
                    }
                }

                // Sort the artifact infos by name, this is just to have a consistent order and make
                // the resolution output consistent.
                for artifact_infos in result.values_mut() {
                    artifact_infos.sort_by(|a, b| a.filename.cmp(&b.filename));
                }

                // Sort in descending order by version
                result.sort_unstable_by(|v1, _, v2, _| v2.cmp(v1));

                Ok(self.artifacts.insert(p.clone(), Box::new(result)))
            }
        }
    }

    /// Writes the metadata for the given artifact into the cache. If the metadata already exists
    /// its not overwritten.
    async fn put_metadata_in_cache(&self, ai: &ArtifactInfo, blob: &[u8]) -> miette::Result<()> {
        if let Some(hash) = &ai.hashes {
            self.metadata_cache
                .get_or_set(&hash, |w| w.write_all(blob))
                .await
                .into_diagnostic()?;
        }
        Ok(())
    }

    /// Fetch metadata by only retrieving chunks of the wheel via HTTP range requests
    pub async fn get_lazy_metadata_wheel(
        &self,
        artifact_info: &ArtifactInfo,
    ) -> miette::Result<Option<WheelCoreMetadata>> {
        tracing::info!(url=%artifact_info.url, "lazy reading artifact");

        // Check if the artifact is the same type as the info.
        let name = WheelFilename::try_as(&artifact_info.filename)
            .expect("the specified artifact does not refer to type requested to read");

        // Construct an async reader
        let Ok((mut reader, _)) = AsyncHttpRangeReader::new(
            self.http.client.clone(),
            artifact_info.url.clone(),
            CheckSupportMethod::Head,
            HeaderMap::default(),
        )
        .await
        else {
            return Ok(None);
        };

        // Try to read the metadata lazily
        match lazy_read_wheel_metadata(name, &mut reader).await {
            Ok((blob, metadata)) => {
                self.put_metadata_in_cache(artifact_info, &blob).await?;
                return Ok(Some(metadata));
            }
            Err(err) => {
                tracing::warn!("failed to sparsely read wheel file: {err}, falling back to downloading the whole file");
            }
        }

        Ok(None)
    }

    /// Retrieve the PEP658 metadata for the given artifact.
    /// This assumes that the metadata is available in the repository
    /// This can be checked with the [`ArtifactInfo`] struct.
    pub async fn get_pep658_metadata<'a, A: Borrow<ArtifactInfo>>(
        &self,
        artifact_info: &'a A,
    ) -> miette::Result<(&'a A, WheelCoreMetadata)> {
        let ai = artifact_info.borrow();

        // Check if the artifact is the same type as the info.
        WheelFilename::try_as(&ai.filename)
            .expect("the specified artifact does not refer to type requested to read");

        // Turn into PEP658 compliant URL
        let mut url = ai.url.clone();
        url.set_path(&url.path().replace(".whl", ".whl.metadata"));

        let mut bytes = Vec::new();
        self.http
            .request(url, Method::GET, HeaderMap::default(), CacheMode::NoStore)
            .await?
            .into_body()
            .read_to_end(&mut bytes)
            .await
            .into_diagnostic()?;

        let metadata = WheelCoreMetadata::try_from(bytes.as_slice()).into_diagnostic()?;
        self.put_metadata_in_cache(ai, &bytes).await?;
        Ok((artifact_info, metadata))
    }

    /// Get all package names in the index.
    pub async fn get_package_names(&self) -> miette::Result<Vec<String>> {
        let index_url = self.sources.default_index_url();
        let response = self
            .http
            .request(
                index_url,
                Method::GET,
                HeaderMap::default(),
                CacheMode::Default,
            )
            .await?;

        let mut bytes = response.into_body().into_local().await.into_diagnostic()?;
        let mut source = String::new();
        bytes.read_to_string(&mut source).into_diagnostic()?;
        parse_package_names_html(&source)
    }
}

async fn fetch_simple_api(
    http: &Http,
    url: Url,
    check_available_artifacts: CheckAvailablePackages,
) -> miette::Result<Option<ProjectInfo>> {
    let mut headers = HeaderMap::new();
    // If we always want to check if there are new available artifacts, we'll set the cache control
    // to max-age=0, so that we always get a non-cached server response.
    if CheckAvailablePackages::Always == check_available_artifacts {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=0"));
    }

    let response = match http
        .request(url.clone(), Method::GET, headers, CacheMode::Default)
        .await
    {
        Ok(response) => response,
        Err(err) => {
            if let HttpRequestError::HttpError(err) = &err {
                if err.status() == Some(StatusCode::NOT_FOUND) {
                    return Ok(None);
                }
            }
            return Err(err.into());
        }
    };

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("text/html")
        .to_owned();

    // Convert the information from html
    let mut bytes = Vec::new();
    response
        .into_body()
        .read_to_end(&mut bytes)
        .await
        .into_diagnostic()?;

    let content_type: mime::Mime = content_type.parse().into_diagnostic()?;
    match (
        content_type.type_().as_str(),
        content_type.subtype().as_str(),
    ) {
        ("text", "html") => {
            parse_project_info_html(&url, std::str::from_utf8(&bytes).into_diagnostic()?).map(Some)
        }
        _ => miette::bail!(
            "simple API page expected Content-Type: text/html, but got {}",
            &content_type
        ),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::types::PackageName;
    use reqwest::Client;
    use tempfile::TempDir;
    use tokio::task::JoinHandle;

    use crate::index::package_sources::PackageSourcesBuilder;
    use axum::response::{Html, IntoResponse};
    use axum::routing::get;
    use axum::Router;
    use insta::assert_debug_snapshot;
    use std::future::IntoFuture;
    use std::net::SocketAddr;
    use tower_http::add_extension::AddExtensionLayer;

    async fn get_index(
        axum::Extension(served_package): axum::Extension<String>,
    ) -> impl IntoResponse {
        // Return the HTML response with the list of packages
        let package_list = format!(
            r#"
            <a href="/{served_package}">{served_package}</a>
        "#
        );

        let html = format!("<html><body>{package_list}</body></html>");
        Html(html)
    }

    async fn get_package(
        axum::Extension(served_package): axum::Extension<String>,
        axum::extract::Path(requested_package): axum::extract::Path<String>,
    ) -> impl IntoResponse {
        if served_package == requested_package {
            let wheel_name = format!("{served_package}-1.0-py3-none-any.whl");
            let link_list = format!(
                r#"
                <a href="/files/{wheel_name}">{wheel_name}</a>
            "#
            );

            let html = format!("<html><body>{link_list}</body></html>");
            Html(html).into_response()
        } else {
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
    }

    async fn make_simple_server(
        package_name: &str,
    ) -> anyhow::Result<(Url, JoinHandle<Result<(), std::io::Error>>)> {
        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let address = listener.local_addr()?;

        let router = Router::new()
            .route("/simple", get(get_index))
            .route("/simple/:package/", get(get_package))
            .layer(AddExtensionLayer::new(package_name.to_string()));

        let server = axum::serve(listener, router).into_future();

        // Spawn the server.
        let join_handle = tokio::spawn(server);

        println!("Server started");
        let url = format!("http://{address}/simple/").parse()?;
        Ok((url, join_handle))
    }

    fn make_package_db() -> (TempDir, PackageDb) {
        let url = Url::parse("https://pypi.org/simple/").unwrap();

        let cache_dir = TempDir::new().unwrap();
        let package_db = PackageDb::new(
            url.into(),
            ClientWithMiddleware::from(Client::new()),
            cache_dir.path(),
            CheckAvailablePackages::default(),
        )
        .unwrap();

        (cache_dir, package_db)
    }

    #[tokio::test]
    async fn test_available_packages() {
        let (_cache_dir, package_db) = make_package_db();
        let name = "scikit-learn".parse::<PackageName>().unwrap();

        // Get all the artifacts
        let artifacts = package_db
            .available_artifacts(ArtifactRequest::FromIndex(name.into()))
            .await
            .unwrap();

        // Get the first wheel artifact
        let _artifact_info = artifacts
            .iter()
            .flat_map(|(_, artifacts)| artifacts.iter().cloned())
            .collect::<Vec<_>>();
    }

    #[tokio::test]
    async fn test_index_mapping() -> anyhow::Result<()> {
        // just a random UUID
        let package_name = "c99d774d1a5a4a7fa2c2820bae6688e7".to_string();

        let (test_index, _server) = make_simple_server(&package_name).await?;
        let pypi_index = Url::parse("https://pypi.org/simple/")?;

        let index_alias = "test-index".to_string();

        let package_name = package_name.parse::<PackageName>()?;
        let normalized_name = NormalizedPackageName::from(package_name);

        let cache_dir = TempDir::new()?;
        let sources = PackageSourcesBuilder::new(pypi_index)
            .with_index(&index_alias, &test_index)
            // Exists in pypi but not in our index
            .with_override("pytest".parse()?, &index_alias)
            // Doesn't exist in pypi (hopefully), should exist in our index
            .with_override(normalized_name.clone(), &index_alias)
            .build()
            .unwrap();

        let package_db = PackageDb::new(
            sources,
            ClientWithMiddleware::from(Client::new()),
            cache_dir.path(),
            CheckAvailablePackages::default(),
        )
        .unwrap();

        let pytest_name = "pytest".parse::<PackageName>()?;
        let pytest_result = package_db
            .available_artifacts(ArtifactRequest::FromIndex(pytest_name.into()))
            .await;

        // Should not fail because 404s are skipped
        assert!(
            pytest_result.is_ok(),
            "`pytest_result` not ok: {pytest_result:?}"
        );

        let test_package_result = package_db
            .available_artifacts(ArtifactRequest::FromIndex(normalized_name))
            .await
            .unwrap();

        assert_debug_snapshot!(test_package_result.keys(), @r#"
        [
            Version {
                version: "1.0",
                package_allows_prerelease: false,
            },
        ]
        "#);

        Ok(())
    }

    #[tokio::test]
    async fn test_pep658() {
        let (_cache_dir, package_db) = make_package_db();
        let name = "scikit-learn".parse::<PackageName>().unwrap();

        // Get all the artifacts
        let artifacts = package_db
            .available_artifacts(ArtifactRequest::FromIndex(name.into()))
            .await
            .unwrap();

        // Get the artifact with dist-info attribute
        let artifact_info = artifacts
            .iter()
            .flat_map(|(_, artifacts)| artifacts.iter())
            // This signifies that a PEP658 metadata file is available
            .find(|a| a.dist_info_metadata.available)
            .unwrap();

        let (_artifact, _metadata) = package_db.get_pep658_metadata(artifact_info).await.unwrap();
    }
}

#[derive(Debug, Diagnostic)]
pub struct NotCached;

impl Display for NotCached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "request not in cache, and cache_mode=OnlyIfCached")
    }
}

impl std::error::Error for NotCached {}
