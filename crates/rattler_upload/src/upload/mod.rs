//! The upload module provides the package upload functionality.

use crate::{
    tool_configuration::APP_USER_AGENT, AnacondaData, ArtifactoryData, CloudsmithData, QuetzData,
};
use fs_err::tokio as fs;
use futures::TryStreamExt;
use indicatif::{style::TemplateError, HumanBytes, ProgressState};
use reqwest_retry::{policies::ExponentialBackoff, RetryDecision, RetryPolicy};
use std::{
    fmt::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio_util::io::ReaderStream;

use miette::{Context, IntoDiagnostic};
use rattler_networking::{Authentication, AuthenticationStorage};
use rattler_redaction::Redact;
use reqwest::{Method, StatusCode};
use tracing::{info, warn};
use url::Url;

use crate::upload::package::{sha256_sum, ExtractedPackage};

#[cfg(test)]
pub(crate) mod test_utils;

mod anaconda;
#[cfg(feature = "sigstore-sign")]
pub mod attestation;
mod cloudsmith;
pub mod conda_forge;
pub mod opt;
mod package;
mod prefix;
#[cfg(feature = "s3")]
mod s3;
mod trusted_publishing;
#[cfg(feature = "s3")]
pub use s3::upload_package_to_s3;

pub use anaconda::AnacondaError;
pub use cloudsmith::CloudsmithError;
pub use prefix::{upload_package_to_prefix, PrefixUploadError};

/// Returns the style to use for a progress bar that is currently in progress.
fn default_bytes_style() -> Result<indicatif::ProgressStyle, TemplateError> {
    Ok(indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} {prefix:20!} [{elapsed_precise}] [{bar:40!.bright.yellow/dim.white}] {bytes:>8} @ {smoothed_bytes_per_sec:8}")?
            .progress_chars("━━╾─")
            .with_key(
                "smoothed_bytes_per_sec",
                |s: &ProgressState, w: &mut dyn Write| match (s.pos(), s.elapsed().as_millis()) {
                    (pos, elapsed_ms) if elapsed_ms > 0 => {
                        // TODO: log with tracing?
                        _ = write!(w, "{}/s", HumanBytes((pos as f64 * 1000_f64 / elapsed_ms as f64) as u64));
                    }
                    _ => {
                        _ = write!(w, "-");
                    },
                },
            ))
}

fn get_default_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .no_gzip()
        .user_agent(APP_USER_AGENT)
        .build()
}

/// Returns a reqwest client with retry middleware.
fn get_client_with_retry() -> Result<reqwest_middleware::ClientWithMiddleware, reqwest::Error> {
    let client = reqwest::Client::builder()
        .no_gzip()
        .user_agent(APP_USER_AGENT)
        .build()?;

    Ok(reqwest_middleware::ClientBuilder::new(client)
        .with(reqwest_retry::RetryTransientMiddleware::new_with_policy(
            reqwest_retry::policies::ExponentialBackoff::builder().build_with_max_retries(3),
        ))
        .build())
}

/// Uploads package files to a Quetz server.
pub async fn upload_package_to_quetz(
    storage: &AuthenticationStorage,
    package_files: &Vec<PathBuf>,
    quetz_data: QuetzData,
) -> miette::Result<()> {
    let token = match quetz_data.api_key {
        Some(api_key) => api_key,
        None => match storage.get_by_url(Url::from(quetz_data.url.clone())) {
            Ok((_, Some(Authentication::CondaToken(token)))) => token,
            Ok((_, Some(_))) => {
                return Err(miette::miette!("A Conda token is required for authentication with quetz.
                        Authentication information found in the keychain / auth file, but it was not a Conda token"));
            }
            Ok((_, None)) => {
                return Err(miette::miette!(
                    "No quetz api key was given and none was found in the keychain / auth file"
                ));
            }
            Err(e) => {
                return Err(miette::miette!(
                    "Failed to get authentication information form keychain: {e}"
                ));
            }
        },
    };

    let client = get_default_client().into_diagnostic()?;

    for package_file in package_files {
        let upload_url = quetz_data
            .url
            .join(&format!(
                "api/channels/{}/upload/{}",
                quetz_data.channels,
                package_file.file_name().unwrap().to_string_lossy()
            ))
            .into_diagnostic()?;

        let hash = sha256_sum(package_file).into_diagnostic()?;

        let prepared_request = client
            .request(Method::POST, upload_url)
            .query(&[("force", "false"), ("sha256", &hash)])
            .header("X-API-Key", token.clone());

        send_request_with_retry(prepared_request, package_file).await?;
    }

    info!("Packages successfully uploaded to Quetz server");

    Ok(())
}

/// Uploads package files to an Artifactory server.
pub async fn upload_package_to_artifactory(
    storage: &AuthenticationStorage,
    package_files: &Vec<PathBuf>,
    artifactory_data: ArtifactoryData,
) -> miette::Result<()> {
    let token = match artifactory_data.token {
        Some(t) => t,
        _ => match storage.get_by_url(Url::from(artifactory_data.url.clone())) {
            Ok((_, Some(Authentication::BearerToken(token)))) => token,
            Ok((
                _,
                Some(Authentication::BasicHTTP {
                    username: _,
                    password,
                }),
            )) => {
                warn!(
                    "A bearer token is required for authentication with artifactory. Using the password from the keychain / auth file to authenticate. Consider switching to a bearer token instead for Artifactory."
                );
                password
            }
            Ok((_, Some(_))) => {
                return Err(miette::miette!("A bearer token is required for authentication with artifactory.
                            Authentication information found in the keychain / auth file, but it was not a bearer token"));
            }
            Ok((_, None)) => {
                return Err(miette::miette!(
                    "No bearer token was given and none was found in the keychain / auth file"
                ));
            }
            Err(e) => {
                return Err(miette::miette!(
                    "Failed to get authentication information form keychain: {e}"
                ));
            }
        },
    };

    for package_file in package_files {
        let package = ExtractedPackage::from_package_file(package_file)?;

        let subdir = package.subdir().ok_or_else(|| {
            miette::miette!(
                "index.json of package {} has no subdirectory. Cannot determine which directory to upload to",
                package_file.display()
            )
        })?;

        let package_name = package.filename().ok_or(miette::miette!(
            "Package file {} has no filename",
            package_file.display()
        ))?;

        let client = get_default_client().into_diagnostic()?;

        let upload_url = artifactory_data
            .url
            .join(&format!(
                "{}/{}/{}",
                artifactory_data.channels, subdir, package_name
            ))
            .into_diagnostic()?;

        let prepared_request = client
            .request(Method::PUT, upload_url)
            .bearer_auth(token.clone());

        send_request_with_retry(prepared_request, package_file).await?;
    }

    info!("Packages successfully uploaded to Artifactory server");

    Ok(())
}

/// Uploads package files to an Anaconda server.
pub async fn upload_package_to_anaconda(
    storage: &AuthenticationStorage,
    package_files: &Vec<PathBuf>,
    anaconda_data: AnacondaData,
) -> Result<(), anaconda::AnacondaError> {
    let token = match anaconda_data.api_key {
        Some(token) => token,
        None => match storage.get_by_url(Url::from(anaconda_data.url.clone())) {
            Ok((_, Some(Authentication::CondaToken(token)))) => token,
            Ok((_, Some(_))) => {
                return Err(anaconda::AnacondaError::WrongAuthenticationType);
            }
            Ok((_, None)) => {
                return Err(anaconda::AnacondaError::MissingApiKey);
            }
            Err(e) => {
                return Err(anaconda::AnacondaError::KeychainError {
                    message: e.to_string(),
                });
            }
        },
    };

    let anaconda = anaconda::Anaconda::new(token, anaconda_data.url);

    for package_file in package_files {
        loop {
            let package = package::ExtractedPackage::from_package_file(package_file)?;

            anaconda
                .create_or_update_package(&anaconda_data.owner, &package)
                .await?;

            anaconda
                .create_or_update_release(&anaconda_data.owner, &package)
                .await?;

            let successful = anaconda
                .upload_file(
                    &anaconda_data.owner,
                    &anaconda_data.channels,
                    anaconda_data.force,
                    &package,
                )
                .await?;

            // When running with --force and experiencing a conflict error, we delete the conflicting file.
            // Anaconda automatically deletes releases / packages when the deletion of a file would leave them empty.
            // Therefore, we need to ensure that the release / package still exists before trying to upload again.
            if successful {
                break;
            }
        }
    }
    Ok(())
}

/// Uploads package files to a Cloudsmith repository.
pub async fn upload_package_to_cloudsmith(
    storage: &AuthenticationStorage,
    package_files: &Vec<PathBuf>,
    cloudsmith_data: CloudsmithData,
) -> Result<(), cloudsmith::CloudsmithError> {
    let token = match cloudsmith_data.api_key {
        Some(token) => token,
        None => match storage.get_by_url(Url::from(cloudsmith_data.url.clone())) {
            Ok((
                _,
                Some(Authentication::CondaToken(token) | Authentication::BearerToken(token)),
            )) => token,
            Ok((_, Some(_))) => {
                return Err(cloudsmith::CloudsmithError::WrongAuthenticationType);
            }
            Ok((_, None)) => {
                return Err(cloudsmith::CloudsmithError::MissingApiKey);
            }
            Err(e) => {
                return Err(cloudsmith::CloudsmithError::KeychainError {
                    message: e.to_string(),
                });
            }
        },
    };

    let client = cloudsmith::Cloudsmith::new(
        token,
        cloudsmith_data.url,
        cloudsmith_data.owner,
        cloudsmith_data.repo,
    );

    for package_file in package_files {
        let package = package::ExtractedPackage::from_package_file(package_file)?;
        let filename = package.filename().ok_or_else(|| {
            miette::miette!("Package file {} has no filename", package_file.display())
        })?;

        let md5 = package.md5_hex().into_diagnostic()?;
        let file_size = package.file_size().into_diagnostic()?;
        let is_multipart = file_size >= cloudsmith::CHUNK_SIZE as u64;

        let upload_response = client.request_upload(filename, &md5, is_multipart).await?;

        if is_multipart {
            client
                .upload_file_multipart(
                    &upload_response.upload_url,
                    &upload_response.identifier,
                    package_file,
                )
                .await?;
        } else {
            client
                .upload_file_single(
                    &upload_response.upload_url,
                    &upload_response.upload_fields,
                    package_file,
                )
                .await?;
        }

        let pkg_response = client.create_package(&upload_response.identifier).await?;
        info!(
            "Package created: slug_perm={}, slug={}",
            pkg_response.slug_perm, pkg_response.slug
        );
    }

    info!("Packages successfully uploaded to Cloudsmith");
    Ok(())
}

async fn send_request_with_retry(
    prepared_request: reqwest::RequestBuilder,
    package_file: &Path,
) -> miette::Result<reqwest::Response> {
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
    let mut current_try = 0;

    let request_start = SystemTime::now();

    loop {
        let request = prepared_request
            .try_clone()
            .expect("Could not clone request. Does it have a streaming body?");
        let response = send_request(request, package_file).await?;

        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let body = response.text().await.into_diagnostic()?;
        let err = miette::miette!(
            "Failed to upload package file: {}\nStatus: {}\nBody: {}",
            package_file.display(),
            status,
            body
        );

        // Non-retry status codes
        match status {
            // Authentication/Authorization errors
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(miette::miette!("Authentication error: {}", err));
            }
            // Resource conflicts
            StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY => {
                return Err(miette::miette!("Resource conflict: {}", err));
            }
            // Client errors
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::PAYLOAD_TOO_LARGE => {
                return Err(miette::miette!("Client error: {}", err));
            }
            _ => {}
        }

        match retry_policy.should_retry(request_start, current_try) {
            RetryDecision::DoNotRetry => {
                return Err(err);
            }
            RetryDecision::Retry { execute_after } => {
                let sleep_for = execute_after
                    .duration_since(SystemTime::now())
                    .unwrap_or(Duration::ZERO);
                warn!(
                    "Failed to upload package file: {}\nStatus: {}\nBody: {}\nRetrying in {} seconds",
                    package_file.display(),
                    status,
                    body,
                    sleep_for.as_secs()
                );
                tokio::time::sleep(sleep_for).await;
            }
        }

        current_try += 1;
    }
}

/// Note that we need to use a regular request. `reqwest_retry` does not support streaming requests.
async fn send_request(
    prepared_request: reqwest::RequestBuilder,
    package_file: &Path,
) -> miette::Result<reqwest::Response> {
    let file = fs::File::open(package_file).await.into_diagnostic()?;

    let file_size = file.metadata().await.into_diagnostic()?.len();
    info!(
        "Uploading package file: {} ({})\n",
        package_file
            .file_name()
            .expect("no filename found")
            .to_string_lossy(),
        HumanBytes(file_size)
    );
    let progress_bar = indicatif::ProgressBar::new(file_size)
        .with_prefix("Uploading")
        .with_style(default_bytes_style().into_diagnostic()?);

    let progress_bar_clone = progress_bar.clone();
    let reader_stream = ReaderStream::new(file)
        .inspect_ok(move |bytes| {
            progress_bar_clone.inc(bytes.len() as u64);
        })
        .inspect_err(|e| {
            println!("Error while uploading: {e}");
        });

    let body = reqwest::Body::wrap_stream(reader_stream);

    let response = prepared_request
        .body(body)
        .send()
        .await
        .map_err(Redact::redact)
        .into_diagnostic()?;

    response
        .error_for_status_ref()
        .map_err(Redact::redact)
        .into_diagnostic()
        .wrap_err("Server responded with error")?;

    progress_bar.finish();
    info!(
        "\nUpload complete for package file: {}",
        package_file
            .file_name()
            .expect("no filename found")
            .to_string_lossy()
    );

    Ok(response)
}

#[cfg(test)]
mod test {
    use axum::{http::StatusCode, Router};
    use rattler_networking::AuthenticationStorage;

    use crate::upload::opt::{ArtifactoryData, QuetzData};
    use crate::upload::test_utils::{start_test_server, test_package_path};

    async fn ok_with_api_key(
        headers: axum::http::HeaderMap,
        _body: axum::body::Bytes,
    ) -> StatusCode {
        assert!(headers.get("x-api-key").is_some());
        StatusCode::OK
    }

    async fn ok_with_bearer(
        headers: axum::http::HeaderMap,
        _body: axum::body::Bytes,
    ) -> StatusCode {
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("Bearer "));
        StatusCode::OK
    }

    async fn unauthorized(_body: axum::body::Bytes) -> StatusCode {
        StatusCode::UNAUTHORIZED
    }

    async fn conflict(_body: axum::body::Bytes) -> StatusCode {
        StatusCode::CONFLICT
    }

    #[tokio::test]
    async fn test_quetz_upload_success() {
        let router = Router::new().fallback(ok_with_api_key);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let quetz_data = QuetzData::new(
            url,
            "test-channel".to_string(),
            Some("test-api-key".to_string()),
        );
        let result =
            super::upload_package_to_quetz(&storage, &vec![test_package_path()], quetz_data).await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_quetz_upload_auth_failure() {
        let router = Router::new().fallback(unauthorized);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let quetz_data =
            QuetzData::new(url, "test-channel".to_string(), Some("bad-key".to_string()));
        let result =
            super::upload_package_to_quetz(&storage, &vec![test_package_path()], quetz_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_quetz_upload_conflict() {
        let router = Router::new().fallback(conflict);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let quetz_data = QuetzData::new(
            url,
            "test-channel".to_string(),
            Some("test-key".to_string()),
        );
        let result =
            super::upload_package_to_quetz(&storage, &vec![test_package_path()], quetz_data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_artifactory_upload_success() {
        let router = Router::new().fallback(ok_with_bearer);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let artifactory_data = ArtifactoryData::new(
            url,
            "test-channel".to_string(),
            Some("test-token".to_string()),
        );
        let result = super::upload_package_to_artifactory(
            &storage,
            &vec![test_package_path()],
            artifactory_data,
        )
        .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_artifactory_upload_auth_failure() {
        let router = Router::new().fallback(unauthorized);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let artifactory_data = ArtifactoryData::new(
            url,
            "test-channel".to_string(),
            Some("bad-token".to_string()),
        );
        let result = super::upload_package_to_artifactory(
            &storage,
            &vec![test_package_path()],
            artifactory_data,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cloudsmith_upload_success() {
        use axum::routing::post;
        use std::net::SocketAddr;

        // Bind the listener first so we know the port for the upload_url response
        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url: url::Url = format!("http://{}:{}", addr.ip(), addr.port())
            .parse()
            .unwrap();

        let upload_handler = {
            let base_url = base_url.clone();
            move |headers: axum::http::HeaderMap| {
                let base_url = base_url.clone();
                async move {
                    assert!(headers.get("X-Api-Key").is_some());
                    let upload_url = base_url.join("s3-upload").unwrap();
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "application/json")],
                        serde_json::json!({
                            "identifier": "test-file-id",
                            "upload_url": upload_url.to_string(),
                            "upload_fields": {"key": "value"}
                        })
                        .to_string(),
                    )
                }
            }
        };

        let router = Router::new()
            .route("/files/{owner}/{repo}/", post(upload_handler))
            .route("/s3-upload", post(|| async { StatusCode::OK }))
            .route(
                "/packages/{owner}/{repo}/upload/conda/",
                post(|| async {
                    (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        serde_json::json!({
                            "slug_perm": "test-slug-perm",
                            "slug": "test-slug"
                        })
                        .to_string(),
                    )
                }),
            );

        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let storage = AuthenticationStorage::empty();
        let cloudsmith_data = crate::upload::opt::CloudsmithData::new(
            "test-owner".to_string(),
            "test-repo".to_string(),
            Some("test-api-key".to_string()),
            Some(base_url),
        );
        let result = super::upload_package_to_cloudsmith(
            &storage,
            &vec![test_package_path()],
            cloudsmith_data,
        )
        .await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_cloudsmith_upload_missing_api_key() {
        let storage = AuthenticationStorage::empty();
        let cloudsmith_data = crate::upload::opt::CloudsmithData::new(
            "test-owner".to_string(),
            "test-repo".to_string(),
            None,
            Some("http://127.0.0.1:1".parse().unwrap()),
        );
        let result = super::upload_package_to_cloudsmith(
            &storage,
            &vec![test_package_path()],
            cloudsmith_data,
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            super::cloudsmith::CloudsmithError::MissingApiKey
        ),);
    }
}
