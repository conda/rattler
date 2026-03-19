use std::borrow::Cow;

use fs_err::tokio as fs;
use miette::IntoDiagnostic;
use rattler_conda_types::package::AboutJson;
use rattler_conda_types::utils::url_with_trailing_slash::UrlWithTrailingSlash;
use rattler_conda_types::PackageName;
use reqwest::multipart::Form;
use reqwest::multipart::Part;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;
use tracing::info;
use url::Url;

use crate::upload::opt::ForceOverwrite;

use super::package::ExtractedPackage;
use super::VERSION;

/// Errors that can occur during Anaconda.org operations.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum AnacondaError {
    /// A conda token is required but a different authentication type was found.
    #[error(
        "conda token required for anaconda.org, but a different authentication type was found"
    )]
    WrongAuthenticationType,

    /// No API key was provided and none was found in the keychain.
    #[error("no anaconda.org API key provided and none found in keychain")]
    MissingApiKey,

    /// Failed to retrieve authentication from the keychain.
    #[error("failed to retrieve authentication from keychain: {message}")]
    KeychainError {
        /// The error message from the keychain.
        message: String,
    },

    /// The server returned an unexpected status code.
    #[error("unexpected server response (HTTP {status})")]
    UnexpectedStatus {
        /// The HTTP status code.
        status: u16,
    },

    /// Failed to create or update a package on the server.
    #[error("failed to create or update package")]
    PackageMutationFailed(#[source] reqwest::Error),

    /// Failed to create or update a release on the server.
    #[error("failed to create or update release")]
    ReleaseMutationFailed(#[source] reqwest::Error),

    /// Failed to remove a file from the server.
    #[error("failed to remove file")]
    FileRemovalFailed(#[source] reqwest::Error),

    /// The file already exists and --force was not specified.
    #[error("file {0} already exists")]
    #[diagnostic(help("use --force to overwrite"))]
    FileAlreadyExists(String),

    /// Failed to stage a file on the server.
    #[error("failed to stage file (HTTP {status})")]
    StageFailed {
        /// The HTTP status code.
        status: u16,
    },

    /// No channel was selected for upload.
    #[error("no channel selected for upload")]
    #[diagnostic(help("specify at least one channel for upload to anaconda.org"))]
    NoChannel,

    /// The index.json is missing the subdir field.
    #[error("missing subdir in index.json")]
    MissingSubdir,

    /// The package file has no filename.
    #[error("missing filename")]
    MissingFilename,

    /// An error from an underlying operation (I/O, URL parsing, etc.).
    #[error("{0}")]
    Other(miette::Report),
}

impl From<miette::Report> for AnacondaError {
    fn from(report: miette::Report) -> Self {
        AnacondaError::Other(report)
    }
}

pub struct Anaconda {
    client: Client,
    url: UrlWithTrailingSlash,
}

#[derive(Serialize, Deserialize, Debug)]
struct PackageAttrs<'a> {
    package_types: Vec<String>,
    name: Cow<'a, PackageName>,
    #[serde(flatten)]
    about: Cow<'a, AboutJson>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ReleaseCreationArgs<'a> {
    requirements: Vec<String>,
    announce: bool,
    description: Option<String>,
    #[serde(flatten)]
    about: Cow<'a, AboutJson>,
}

#[derive(Serialize, Deserialize, Debug)]
struct FileStageResponse {
    post_url: Url,
    form_data: serde_json::Map<String, serde_json::Value>,
    dist_id: String,
}

impl Anaconda {
    pub fn new(token: String, url: UrlWithTrailingSlash) -> Self {
        let mut default_headers = reqwest::header::HeaderMap::new();

        default_headers.append(
            "Accept",
            "application/json".parse().expect("failed to parse"),
        );
        default_headers.append(
            "Authorization",
            format!("token {token}").parse().expect("failed to parse"),
        );

        default_headers.append(
            "x-binstar-api-version",
            "1.12.2".parse().expect("failed to parse"),
        );

        let client = Client::builder()
            .no_gzip()
            .user_agent(format!("rattler-build/{VERSION}"))
            .default_headers(default_headers)
            .build()
            .expect("failed to create client");

        Self { client, url }
    }
}

impl Anaconda {
    pub async fn create_or_update_package(
        &self,
        owner: &str,
        package: &ExtractedPackage<'_>,
    ) -> Result<(), AnacondaError> {
        let package_name = package.package_name();
        debug!("getting package {}/{}", owner, package_name.as_normalized(),);

        let url = self
            .url
            .join(&format!(
                "package/{}/{}",
                owner,
                package_name.as_normalized(),
            ))
            .into_diagnostic()?;

        let response = self.client.get(url).send().await.into_diagnostic()?;

        let exists = match response.status() {
            reqwest::StatusCode::OK => true,
            reqwest::StatusCode::NOT_FOUND => false,
            status => {
                return Err(AnacondaError::UnexpectedStatus {
                    status: status.as_u16(),
                });
            }
        };

        let url = self
            .url
            .join(&format!(
                "package/{}/{}",
                owner,
                package_name.as_normalized(),
            ))
            .into_diagnostic()?;

        // See inspect_conda_info_dir in anaconda-client
        // https://github.com/Anaconda-Platform/anaconda-client/blob/master/binstar_client/inspect_package/conda.py#L81-L150
        // dumping the entire about.json as public_attrs seems to work fine
        let payload = serde_json::json!({
            "public": true,
            "publish": false,
            "public_attrs": PackageAttrs {
                package_types: vec!["conda".to_string()],
                name: Cow::Borrowed(package_name),
                about: Cow::Borrowed(package.about_json()),
            },
        });

        let req = if exists {
            debug!(
                "updating package {}/{}",
                owner,
                package_name.as_normalized(),
            );
            self.client.patch(url)
        } else {
            debug!(
                "creating package {}/{}",
                owner,
                package_name.as_normalized(),
            );
            self.client.post(url)
        };

        req.json(&payload)
            .send()
            .await
            .map_err(AnacondaError::PackageMutationFailed)?
            .error_for_status()
            .map_err(AnacondaError::PackageMutationFailed)?;

        Ok(())
    }

    pub async fn create_or_update_release(
        &self,
        owner: &str,
        package: &ExtractedPackage<'_>,
    ) -> Result<(), AnacondaError> {
        let package_name = package.package_name();
        let package_version = package.package_version();
        debug!(
            "getting release {}/{}/{}",
            owner,
            package_name.as_normalized(),
            package_version
        );

        let url = self
            .url
            .join(&format!(
                "release/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version,
            ))
            .into_diagnostic()?;

        let response = self.client.get(url).send().await.into_diagnostic()?;

        let exists = match response.status() {
            reqwest::StatusCode::OK => true,
            reqwest::StatusCode::NOT_FOUND => false,
            status => {
                return Err(AnacondaError::UnexpectedStatus {
                    status: status.as_u16(),
                });
            }
        };

        let url = self
            .url
            .join(&format!(
                "release/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version,
            ))
            .into_diagnostic()?;

        let req = if exists {
            debug!(
                "updating release {}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version
            );
            self.client.patch(url).json(&serde_json::json!({
                "requirements": [],
                "announce": false,
                "description": null,
                "public_attrs": Cow::Borrowed(package.about_json())
            }))
        } else {
            debug!(
                "creating release {}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version
            );
            self.client.post(url).json(&ReleaseCreationArgs {
                requirements: vec![],
                announce: false,
                description: None,
                about: Cow::Borrowed(package.about_json()),
            })
        };

        req.send()
            .await
            .map_err(AnacondaError::ReleaseMutationFailed)?
            .error_for_status()
            .map_err(AnacondaError::ReleaseMutationFailed)?;

        Ok(())
    }

    pub async fn remove_file(
        &self,
        owner: &str,
        package: &ExtractedPackage<'_>,
    ) -> Result<(), AnacondaError> {
        let package_name = package.package_name();
        let package_version = package.package_version();
        let subdir = package.subdir().ok_or(AnacondaError::MissingSubdir)?;
        let filename = package.filename().ok_or(AnacondaError::MissingFilename)?;

        debug!(
            "removing file {}/{}/{}/{}/{}",
            owner,
            package_name.as_normalized(),
            package_version,
            subdir,
            filename,
        );

        let url = self
            .url
            .join(&format!(
                "dist/{}/{}/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                package_version,
                subdir,
                filename,
            ))
            .into_diagnostic()?;

        self.client
            .delete(url)
            .send()
            .await
            .map_err(AnacondaError::FileRemovalFailed)?
            .error_for_status()
            .map_err(AnacondaError::FileRemovalFailed)?;

        Ok(())
    }

    pub async fn upload_file(
        &self,
        owner: &str,
        channels: &[String],
        force: ForceOverwrite,
        package: &ExtractedPackage<'_>,
    ) -> Result<bool, AnacondaError> {
        if channels.is_empty() {
            return Err(AnacondaError::NoChannel);
        }

        let sha256 = package.sha256().into_diagnostic()?;

        let package_name = package.package_name();
        let version = package.package_version();

        let index_json = &package.index_json();

        let subdir = index_json
            .subdir
            .as_deref()
            .ok_or(AnacondaError::MissingSubdir)?;

        let filename = package.filename().ok_or(AnacondaError::MissingFilename)?;

        debug!(
            "uploading file {}/{}/{}/{}/{}",
            owner,
            package_name.as_normalized(),
            version,
            subdir,
            filename,
        );

        let url = self
            .url
            .join(&format!(
                "stage/{}/{}/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                version,
                subdir,
                filename,
            ))
            .into_diagnostic()?;

        let payload = serde_json::json!({
            "distribution_type": "conda",
            "description": null,
            "attrs": index_json,
            "channels": channels,
            "sha256": sha256,
        });

        let resp = self
            .client
            .post(url)
            .json(&payload)
            .send()
            .await
            .into_diagnostic()?;

        match resp.status() {
            reqwest::StatusCode::OK => (),
            reqwest::StatusCode::CONFLICT => {
                if force.is_enabled() {
                    info!(
                        "file {} already exists, running with --force, removing file and retrying",
                        filename
                    );
                    self.remove_file(owner, package).await?;

                    // We cannot just retry the staging request here, because
                    // Anaconda might have garbage collected the release /
                    // package after the deletion of the file.
                    return Ok(false);
                } else {
                    return Err(AnacondaError::FileAlreadyExists(filename.to_string()));
                }
            }
            status => {
                return Err(AnacondaError::StageFailed {
                    status: status.as_u16(),
                });
            }
        }

        let parsed_response: FileStageResponse = resp.json().await.into_diagnostic()?;

        debug!("Uploading file to S3 Bucket {}", parsed_response.post_url);

        let md5_base64 = package.md5_base64().into_diagnostic()?;
        let file_size = package.file_size().into_diagnostic()?;

        let mut form_data = Form::new();

        for (key, value) in parsed_response.form_data {
            let serde_json::Value::String(value) = value else {
                return Err(miette::miette!("invalid value in form data: {}", value).into());
            };

            form_data = form_data.text(key, value);
        }

        let content = fs::read(package.path()).await.into_diagnostic()?;

        form_data = form_data.text("Content-Length", file_size.to_string());
        form_data = form_data.text("Content-MD5", md5_base64);
        form_data = form_data.part("file", Part::bytes(content));

        reqwest::Client::new()
            .post(parsed_response.post_url)
            .multipart(form_data)
            .header("Accept", "application/json")
            .send()
            .await
            .into_diagnostic()?
            .error_for_status()
            .into_diagnostic()?;

        debug!("Committing file {}", filename);

        let url = self
            .url
            .join(&format!(
                "commit/{}/{}/{}/{}/{}",
                owner,
                package_name.as_normalized(),
                version,
                subdir,
                filename,
            ))
            .into_diagnostic()?;

        self.client
            .post(url)
            .json(&serde_json::json!({
                "dist_id": parsed_response.dist_id,
            }))
            .send()
            .await
            .into_diagnostic()?
            .error_for_status()
            .into_diagnostic()?;

        debug!("File {} uploaded successfully", filename);

        Ok(true)
    }
}

#[cfg(test)]
mod test {
    use std::net::SocketAddr;

    use axum::{
        http::StatusCode,
        routing::{get, post},
        Router,
    };
    use url::Url;

    use super::{Anaconda, AnacondaError};
    use crate::upload::package::ExtractedPackage;
    use crate::upload::test_utils::test_package_path;
    use crate::upload::{opt::ForceOverwrite, test_utils};

    #[tokio::test]
    async fn test_anaconda_create_package() {
        let router = Router::new().route(
            "/package/{owner}/{name}",
            get(|| async { StatusCode::NOT_FOUND }).post(|| async { StatusCode::OK }),
        );
        let url = test_utils::start_test_server(router).await;
        let anaconda = Anaconda::new("test-token".to_string(), url.into());

        let package_path = test_package_path();
        let package = ExtractedPackage::from_package_file(&package_path).unwrap();

        let result = anaconda
            .create_or_update_package("test-owner", &package)
            .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_anaconda_update_existing_package() {
        let router = Router::new().route(
            "/package/{owner}/{name}",
            get(|| async { StatusCode::OK }).patch(|| async { StatusCode::OK }),
        );
        let url = test_utils::start_test_server(router).await;
        let anaconda = Anaconda::new("test-token".to_string(), url.into());

        let package_path = test_package_path();
        let package = ExtractedPackage::from_package_file(&package_path).unwrap();

        let result = anaconda
            .create_or_update_package("test-owner", &package)
            .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_anaconda_create_package_server_error() {
        let router = Router::new().route(
            "/package/{owner}/{name}",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let url = test_utils::start_test_server(router).await;
        let anaconda = Anaconda::new("test-token".to_string(), url.into());

        let package_path = test_package_path();
        let package = ExtractedPackage::from_package_file(&package_path).unwrap();

        let err = anaconda
            .create_or_update_package("test-owner", &package)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AnacondaError::UnexpectedStatus { status: 500 }),
            "expected UnexpectedStatus(500), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_anaconda_upload_file_conflict_without_force() {
        let router = Router::new().route(
            "/stage/{owner}/{name}/{version}/{subdir}/{filename}",
            post(|| async { StatusCode::CONFLICT }),
        );
        let url = test_utils::start_test_server(router).await;
        let anaconda = Anaconda::new("test-token".to_string(), url.into());

        let package_path = test_package_path();
        let package = ExtractedPackage::from_package_file(&package_path).unwrap();

        let err = anaconda
            .upload_file(
                "test-owner",
                &["main".to_string()],
                ForceOverwrite(false),
                &package,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, AnacondaError::FileAlreadyExists(..)),
            "expected FileAlreadyExists, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_anaconda_upload_file_no_channels() {
        let package_path = test_package_path();
        let package = ExtractedPackage::from_package_file(&package_path).unwrap();

        let url: Url = "http://127.0.0.1:1".parse().unwrap();
        let anaconda = Anaconda::new("test-token".to_string(), url.into());

        let err = anaconda
            .upload_file("test-owner", &[], ForceOverwrite(false), &package)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AnacondaError::NoChannel),
            "expected NoChannel, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_anaconda_upload_file_full_flow() {
        // Bind the listener first so we know the port for the stage response's post_url
        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url: Url = format!("http://{}:{}", addr.ip(), addr.port())
            .parse()
            .unwrap();

        let stage_handler = {
            let base_url = base_url.clone();
            move || {
                let base_url = base_url.clone();
                async move {
                    let post_url = base_url.join("s3-upload").unwrap();
                    let body = serde_json::json!({
                        "post_url": post_url.to_string(),
                        "form_data": {"key": "value"},
                        "dist_id": "test-dist-id"
                    });
                    (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        body.to_string(),
                    )
                }
            }
        };

        let router = Router::new()
            .route(
                "/package/{owner}/{name}",
                get(|| async { StatusCode::NOT_FOUND }).post(|| async { StatusCode::OK }),
            )
            .route(
                "/release/{owner}/{name}/{version}",
                get(|| async { StatusCode::NOT_FOUND }).post(|| async { StatusCode::OK }),
            )
            .route(
                "/stage/{owner}/{name}/{version}/{subdir}/{filename}",
                post(stage_handler),
            )
            .route("/s3-upload", post(|| async { StatusCode::OK }))
            .route(
                "/commit/{owner}/{name}/{version}/{subdir}/{filename}",
                post(|| async { StatusCode::OK }),
            );

        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let anaconda = Anaconda::new("test-token".to_string(), base_url.into());

        let package_path = test_package_path();
        let package = ExtractedPackage::from_package_file(&package_path).unwrap();

        anaconda
            .create_or_update_package("test-owner", &package)
            .await
            .expect("create_or_update_package failed");

        anaconda
            .create_or_update_release("test-owner", &package)
            .await
            .expect("create_or_update_release failed");

        let result = anaconda
            .upload_file(
                "test-owner",
                &["main".to_string()],
                ForceOverwrite(false),
                &package,
            )
            .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
        assert!(result.unwrap(), "upload_file should return true on success");
    }
}
