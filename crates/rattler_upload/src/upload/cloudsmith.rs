use miette::IntoDiagnostic;
use rattler_conda_types::utils::url_with_trailing_slash::UrlWithTrailingSlash;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info};

use crate::tool_configuration::APP_USER_AGENT;

/// Chunk size for multi-part uploads (100 MB).
pub const CHUNK_SIZE: usize = 1024 * 1024 * 100;

/// Errors that can occur during Cloudsmith operations.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum CloudsmithError {
    /// An API key or bearer token is required but a different authentication type was found.
    #[error(
        "API key or bearer token required for Cloudsmith, but a different authentication type was found"
    )]
    WrongAuthenticationType,

    /// No API key was provided and none was found in the keychain.
    #[error("no Cloudsmith API key provided and none found in keychain")]
    MissingApiKey,

    /// Failed to retrieve authentication from the keychain.
    #[error("failed to retrieve authentication from keychain: {message}")]
    KeychainError {
        /// The error message from the keychain.
        message: String,
    },

    /// Failed to request a file upload slot.
    #[error("failed to request file upload (HTTP {status}): {body}")]
    UploadRequestFailed {
        /// The HTTP status code.
        status: u16,
        /// The response body.
        body: String,
    },

    /// Failed to upload file data.
    #[error("failed to upload file data")]
    UploadFailed(#[source] reqwest::Error),

    /// Failed to complete a multi-part upload.
    #[error("failed to complete multi-part upload (HTTP {status}): {body}")]
    UploadCompleteFailed {
        /// The HTTP status code.
        status: u16,
        /// The response body.
        body: String,
    },

    /// Failed to create the package.
    #[error("failed to create package (HTTP {status}): {body}")]
    PackageCreationFailed {
        /// The HTTP status code.
        status: u16,
        /// The response body.
        body: String,
    },

    /// An error from an underlying operation (I/O, URL parsing, etc.).
    #[error("{0}")]
    Other(miette::Report),
}

impl From<miette::Report> for CloudsmithError {
    fn from(report: miette::Report) -> Self {
        CloudsmithError::Other(report)
    }
}

/// Response from the Cloudsmith file upload request endpoint.
#[derive(Deserialize, Debug)]
pub struct UploadResponse {
    /// The file identifier to use when creating the package.
    pub identifier: String,
    /// The pre-signed URL for uploading file data.
    pub upload_url: String,
    /// Form fields required for single-part upload (S3 pre-signed POST).
    pub upload_fields: serde_json::Map<String, serde_json::Value>,
}

/// Response from the Cloudsmith package creation endpoint.
#[derive(Deserialize, Debug)]
pub struct PackageResponse {
    /// The permanent slug identifier.
    pub slug_perm: String,
    /// The URL-friendly slug.
    pub slug: String,
}

pub struct Cloudsmith {
    client: Client,
    api_key: String,
    url: UrlWithTrailingSlash,
    owner: String,
    repo: String,
}

impl Cloudsmith {
    pub fn new(api_key: String, url: UrlWithTrailingSlash, owner: String, repo: String) -> Self {
        let mut default_headers = reqwest::header::HeaderMap::new();
        default_headers.append(
            "Accept",
            "application/json".parse().expect("failed to parse"),
        );
        default_headers.append(
            "X-Api-Key",
            api_key.parse().expect("failed to parse API key header"),
        );

        let client = Client::builder()
            .no_gzip()
            .user_agent(APP_USER_AGENT)
            .default_headers(default_headers)
            .build()
            .expect("failed to create client");

        Self {
            client,
            api_key,
            url,
            owner,
            repo,
        }
    }

    /// Request an upload slot from Cloudsmith. Returns the file identifier,
    /// pre-signed upload URL, and any required form fields.
    pub async fn request_upload(
        &self,
        filename: &str,
        md5_checksum: &str,
        is_multipart: bool,
    ) -> Result<UploadResponse, CloudsmithError> {
        let url = self
            .url
            .join(&format!("files/{}/{}/", self.owner, self.repo))
            .into_diagnostic()?;

        let method = if is_multipart { "put_parts" } else { "post" };

        debug!("requesting upload slot for {filename}");

        let resp = self
            .client
            .post(url)
            .json(&serde_json::json!({
                "filename": filename,
                "md5_checksum": md5_checksum,
                "method": method,
            }))
            .send()
            .await
            .into_diagnostic()?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.into_diagnostic()?;
            return Err(CloudsmithError::UploadRequestFailed { status, body });
        }

        let upload_resp: UploadResponse = resp.json().await.into_diagnostic()?;
        debug!(
            "upload slot assigned for {filename}: identifier={}",
            upload_resp.identifier
        );
        Ok(upload_resp)
    }

    /// Upload file data to a pre-signed URL (single-part, for files < 100 MB).
    pub async fn upload_file_single(
        &self,
        upload_url: &str,
        upload_fields: &serde_json::Map<String, serde_json::Value>,
        file_path: &std::path::Path,
    ) -> Result<(), CloudsmithError> {
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("package");

        debug!("uploading {filename} (single-part) to pre-signed URL");

        let mut form = Form::new();

        for (key, value) in upload_fields {
            let serde_json::Value::String(value) = value else {
                return Err(miette::miette!("invalid value in upload_fields: {}", value).into());
            };
            form = form.text(key.clone(), value.clone());
        }

        let content = fs_err::tokio::read(file_path).await.into_diagnostic()?;
        form = form.part("file", Part::bytes(content).file_name(filename.to_string()));

        let resp = reqwest::Client::new()
            .post(upload_url)
            .multipart(form)
            .send()
            .await
            .map_err(CloudsmithError::UploadFailed)?;

        resp.error_for_status()
            .map_err(CloudsmithError::UploadFailed)?;

        debug!("single-part upload complete for {filename}");
        Ok(())
    }

    /// Upload file data in chunks (multi-part, for files >= 100 MB).
    pub async fn upload_file_multipart(
        &self,
        upload_url: &str,
        upload_id: &str,
        file_path: &std::path::Path,
    ) -> Result<(), CloudsmithError> {
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("package");

        debug!("uploading {filename} (multi-part) to pre-signed URL");

        let content = fs_err::tokio::read(file_path).await.into_diagnostic()?;
        let total_chunks = content.len().div_ceil(CHUNK_SIZE);
        info!(
            "uploading {filename} ({} bytes, {total_chunks} chunks)",
            content.len()
        );
        let mut chunk_number: usize = 1;

        for chunk in content.chunks(CHUNK_SIZE) {
            debug!("uploading chunk {chunk_number}/{total_chunks} for {filename}");

            let resp = reqwest::Client::new()
                .put(upload_url)
                .header("X-Api-Key", &self.api_key)
                .query(&[
                    ("upload_id", upload_id),
                    ("part_number", &chunk_number.to_string()),
                ])
                .body(chunk.to_vec())
                .send()
                .await
                .map_err(CloudsmithError::UploadFailed)?;

            resp.error_for_status()
                .map_err(CloudsmithError::UploadFailed)?;

            chunk_number += 1;
        }

        // Complete the multi-part upload
        self.complete_upload(upload_id).await?;

        debug!("multi-part upload complete for {filename}");
        Ok(())
    }

    /// Signal to Cloudsmith that a multi-part upload is complete.
    async fn complete_upload(&self, upload_id: &str) -> Result<(), CloudsmithError> {
        let url = self
            .url
            .join(&format!("files/{}/{}/complete/", self.owner, self.repo))
            .into_diagnostic()?;

        debug!("completing multi-part upload {upload_id}");

        let resp = self
            .client
            .post(url)
            .json(&serde_json::json!({
                "upload_id": upload_id,
                "complete": true,
            }))
            .send()
            .await
            .into_diagnostic()?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.into_diagnostic()?;
            return Err(CloudsmithError::UploadCompleteFailed { status, body });
        }

        debug!("multi-part upload {upload_id} completed successfully");
        Ok(())
    }

    /// Create a conda package from a previously uploaded file.
    pub async fn create_package(
        &self,
        file_identifier: &str,
    ) -> Result<PackageResponse, CloudsmithError> {
        let url = self
            .url
            .join(&format!(
                "packages/{}/{}/upload/conda/",
                self.owner, self.repo
            ))
            .into_diagnostic()?;

        info!("creating conda package in {}/{}", self.owner, self.repo);

        let resp = self
            .client
            .post(url)
            .json(&serde_json::json!({
                "package_file": file_identifier,
            }))
            .send()
            .await
            .into_diagnostic()?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.into_diagnostic()?;
            return Err(CloudsmithError::PackageCreationFailed { status, body });
        }

        resp.json().await.into_diagnostic().map_err(Into::into)
    }
}

#[cfg(test)]
mod test {
    use std::net::SocketAddr;

    use axum::{http::StatusCode, routing::post, Router};
    use url::Url;

    use super::Cloudsmith;

    #[tokio::test]
    async fn test_cloudsmith_client_sends_api_key_header() {
        let router = Router::new().fallback(|headers: axum::http::HeaderMap| async move {
            let api_key = headers.get("X-Api-Key").unwrap().to_str().unwrap();
            assert_eq!(api_key, "test-api-key");
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                serde_json::json!({
                    "identifier": "test-id",
                    "upload_url": "http://localhost/upload",
                    "upload_fields": {"key": "value"}
                })
                .to_string(),
            )
        });

        let url = crate::upload::test_utils::start_test_server(router).await;
        let client = Cloudsmith::new(
            "test-api-key".to_string(),
            url.into(),
            "test-owner".to_string(),
            "test-repo".to_string(),
        );

        let result = client
            .request_upload("test.conda", "d41d8cd98f00b204e9800998ecf8427e", false)
            .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_cloudsmith_request_upload_failure() {
        let router = Router::new().fallback(|| async { StatusCode::UNAUTHORIZED });

        let url = crate::upload::test_utils::start_test_server(router).await;
        let client = Cloudsmith::new(
            "bad-key".to_string(),
            url.into(),
            "test-owner".to_string(),
            "test-repo".to_string(),
        );

        let result = client
            .request_upload("test.conda", "d41d8cd98f00b204e9800998ecf8427e", false)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cloudsmith_multipart_upload_flow() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url: Url = format!("http://{}:{}", addr.ip(), addr.port())
            .parse()
            .unwrap();

        let chunk_count = Arc::new(AtomicUsize::new(0));
        let chunk_count_clone = chunk_count.clone();

        let upload_handler = {
            let base_url = base_url.clone();
            move || {
                let base_url = base_url.clone();
                async move {
                    let upload_url = base_url.join("s3-upload").unwrap();
                    (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        serde_json::json!({
                            "identifier": "multipart-test-id",
                            "upload_url": upload_url.to_string(),
                            "upload_fields": {}
                        })
                        .to_string(),
                    )
                }
            }
        };

        let router = Router::new()
            .route("/files/{owner}/{repo}/", post(upload_handler))
            .route(
                "/s3-upload",
                axum::routing::put({
                    let chunk_count = chunk_count_clone;
                    move || {
                        let chunk_count = chunk_count.clone();
                        async move {
                            chunk_count.fetch_add(1, Ordering::SeqCst);
                            StatusCode::OK
                        }
                    }
                }),
            )
            .route(
                "/files/{owner}/{repo}/complete/",
                post(|| async { StatusCode::OK }),
            )
            .route(
                "/packages/{owner}/{repo}/upload/conda/",
                post(|| async {
                    (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        serde_json::json!({
                            "slug_perm": "mp-slug-perm",
                            "slug": "mp-slug"
                        })
                        .to_string(),
                    )
                }),
            );

        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let client = Cloudsmith::new(
            "test-api-key".to_string(),
            base_url.into(),
            "test-owner".to_string(),
            "test-repo".to_string(),
        );

        let package_path = crate::upload::test_utils::test_package_path();

        let upload_resp = client
            .request_upload("test.conda", "d41d8cd98f00b204e9800998ecf8427e", true)
            .await
            .expect("request_upload failed");

        assert_eq!(upload_resp.identifier, "multipart-test-id");

        let result = client
            .upload_file_multipart(
                &upload_resp.upload_url,
                &upload_resp.identifier,
                &package_path,
            )
            .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());

        // The test package is small, so we expect exactly 1 chunk
        assert!(chunk_count.load(Ordering::SeqCst) >= 1);

        let pkg = client
            .create_package(&upload_resp.identifier)
            .await
            .expect("create_package failed");
        assert_eq!(pkg.slug_perm, "mp-slug-perm");
    }
}
