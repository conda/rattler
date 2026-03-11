use fs_err::tokio as tokio_fs;
use futures::TryStreamExt as _;
use miette::IntoDiagnostic as _;
use rattler_networking::{Authentication, AuthenticationStorage};
use reqwest::{
    header::{self, HeaderMap, HeaderValue},
    StatusCode,
};
use reqwest_retry::{policies::ExponentialBackoff, RetryDecision, RetryPolicy};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio_util::io::ReaderStream;
use tracing::{info, warn};
use url::Url;

use super::opt::{AttestationSource, PrefixData};

#[cfg(feature = "sigstore-sign")]
use crate::upload::attestation::{create_attestation, AttestationConfig};
use crate::upload::{
    default_bytes_style, get_client_with_retry, get_default_client,
    trusted_publishing::{check_trusted_publishing, TrustedPublishResult},
};

use super::package::sha256_sum;

/// Errors that can occur during prefix.dev package upload.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum PrefixUploadError {
    /// A bearer token is required but a different authentication type was found.
    #[error("bearer token required for prefix.dev, but a different authentication type was found")]
    WrongAuthenticationType,

    /// No API key was provided and none was found in the keychain.
    #[error("no prefix.dev API key provided and none found in keychain")]
    MissingApiKey,

    /// Failed to retrieve authentication from the keychain.
    #[error("failed to retrieve authentication from keychain: {message}")]
    KeychainError {
        /// The error message from the keychain.
        message: String,
    },

    /// Attestation generation was requested but the sigstore-sign feature is not enabled.
    #[error("attestation generation requested but the sigstore-sign feature is not enabled")]
    #[diagnostic(help("rebuild with the 'sigstore-sign' feature enabled"))]
    AttestationNotAvailable,

    /// Attestation was requested but trusted publishing is not configured.
    #[error("attestation requested but trusted publishing is not configured")]
    AttestationRequiresTrustedPublishing,

    /// The server returned an authentication error (HTTP 401 or 403).
    #[error("authentication failed (HTTP {status})")]
    AuthenticationFailed {
        /// The HTTP status code.
        status: u16,
        /// The response body.
        body: String,
    },

    /// The package already exists on the server (HTTP 409).
    #[error("package already exists (HTTP 409)")]
    Conflict {
        /// The response body.
        body: String,
    },

    /// The server returned an unprocessable entity error (HTTP 422).
    #[error("unprocessable entity (HTTP 422)")]
    UnprocessableEntity {
        /// The response body.
        body: String,
    },

    /// The server returned a client error (HTTP 400, 404, or 413).
    #[error("client error (HTTP {status})")]
    ClientError {
        /// The HTTP status code.
        status: u16,
        /// The response body.
        body: String,
    },

    /// The upload failed after exhausting retries.
    #[error("upload failed after retries (HTTP {status})")]
    ServerError {
        /// The HTTP status code.
        status: u16,
        /// The response body.
        body: String,
    },

    /// An error from an underlying operation (I/O, URL parsing, etc.).
    #[error("{0}")]
    Other(miette::Report),
}

impl From<miette::Report> for PrefixUploadError {
    fn from(report: miette::Report) -> Self {
        PrefixUploadError::Other(report)
    }
}

async fn create_upload_form(
    package_file: &Path,
    filename: &str,
    file_size: u64,
    progress_bar: indicatif::ProgressBar,
    attestation: &Option<PathBuf>,
) -> miette::Result<reqwest::multipart::Form> {
    let mut form = reqwest::multipart::Form::new();

    let progress_bar_clone = progress_bar.clone();
    let reader_stream = ReaderStream::new(
        tokio_fs::File::open(package_file).await.into_diagnostic()?,
    )
    .inspect_ok(move |bytes| {
        progress_bar_clone.inc(bytes.len() as u64);
    });

    let hash = sha256_sum(package_file).into_diagnostic()?;

    let mut file_headers = HeaderMap::new();
    file_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    file_headers.insert(
        header::CONTENT_LENGTH,
        file_size.to_string().parse().into_diagnostic()?,
    );
    file_headers.insert("X-File-Name", filename.parse().unwrap());
    file_headers.insert("X-File-SHA256", hash.parse().unwrap());

    let file_part = reqwest::multipart::Part::stream_with_length(
        reqwest::Body::wrap_stream(reader_stream),
        file_size,
    )
    .file_name(filename.to_owned())
    .headers(file_headers);

    form = form.part("file", file_part);

    if let Some(attestation) = attestation {
        let text = tokio_fs::read_to_string(attestation)
            .await
            .into_diagnostic()?;
        form = form.part("attestation", reqwest::multipart::Part::text(text));
    }

    Ok(form)
}

/// Uploads package files to a prefix.dev server.
pub async fn upload_package_to_prefix(
    storage: &AuthenticationStorage,
    package_files: &Vec<PathBuf>,
    prefix_data: PrefixData,
) -> Result<(), PrefixUploadError> {
    let check_storage = || match storage.get_by_url(Url::from(prefix_data.url.clone())) {
        Ok((_, Some(Authentication::BearerToken(token)))) => Ok(token),
        Ok((_, Some(_))) => Err(PrefixUploadError::WrongAuthenticationType),
        Ok((_, None)) => Err(PrefixUploadError::MissingApiKey),
        Err(e) => Err(PrefixUploadError::KeychainError {
            message: e.to_string(),
        }),
    };

    let client = get_client_with_retry().into_diagnostic()?;

    let wants_attestation = !matches!(prefix_data.attestation, AttestationSource::NoAttestation);
    let wants_generate = matches!(
        prefix_data.attestation,
        AttestationSource::GenerateAttestation
    );

    // Check if attestation generation is requested but sigstore feature is not enabled
    #[cfg(not(feature = "sigstore-sign"))]
    if wants_generate {
        return Err(PrefixUploadError::AttestationNotAvailable);
    }

    // Check if we're using trusted publishing and if we should generate attestations
    #[cfg(feature = "sigstore-sign")]
    let (token, should_generate_attestation) = match prefix_data.api_key {
        Some(api_key) => (api_key, false),
        None => match check_trusted_publishing(&client, &prefix_data.url).await {
            TrustedPublishResult::Configured(token) => {
                // When using trusted publishing, we can generate attestations
                // Note: sigstore-sign handles OIDC token retrieval internally
                (token.secret().to_string(), wants_generate)
            }
            TrustedPublishResult::Skipped => {
                if wants_attestation {
                    return Err(PrefixUploadError::AttestationRequiresTrustedPublishing);
                }
                (check_storage()?, false)
            }
            TrustedPublishResult::Ignored(err) => {
                tracing::warn!("Checked for trusted publishing but failed with {err}");
                if wants_attestation {
                    return Err(PrefixUploadError::AttestationRequiresTrustedPublishing);
                }
                (check_storage()?, false)
            }
        },
    };

    #[cfg(not(feature = "sigstore-sign"))]
    let token = match prefix_data.api_key {
        Some(api_key) => api_key,
        None => match check_trusted_publishing(&client, &prefix_data.url).await {
            TrustedPublishResult::Configured(token) => token.secret().to_string(),
            TrustedPublishResult::Skipped => {
                if wants_attestation {
                    return Err(PrefixUploadError::AttestationRequiresTrustedPublishing);
                }
                check_storage()?
            }
            TrustedPublishResult::Ignored(err) => {
                tracing::warn!("Checked for trusted publishing but failed with {err}");
                if wants_attestation {
                    return Err(PrefixUploadError::AttestationRequiresTrustedPublishing);
                }
                check_storage()?
            }
        },
    };

    for package_file in package_files {
        let filename = package_file
            .file_name()
            .expect("no filename found")
            .to_string_lossy()
            .to_string();
        let file_size = package_file.metadata().into_diagnostic()?.len();
        let mut url = prefix_data
            .url
            .join(&format!("api/v1/upload/{}", prefix_data.channel))
            .into_diagnostic()?;

        if prefix_data.force.is_enabled() {
            url.query_pairs_mut().append_pair("force", "true");
        }

        // Generate attestation if we're using trusted publishing and it was requested
        #[cfg(feature = "sigstore-sign")]
        let attestation_path = if should_generate_attestation {
            let channel_url = prefix_data
                .url
                .join(&prefix_data.channel)
                .into_diagnostic()?;

            // Build attestation configuration
            let config = if prefix_data.store_github_attestation {
                // Parse GITHUB_REPOSITORY (format: "owner/repo")
                let (repo_owner, repo_name) = std::env::var("GITHUB_REPOSITORY")
                    .ok()
                    .and_then(|repo| {
                        let parts: Vec<&str> = repo.splitn(2, '/').collect();
                        if parts.len() == 2 {
                            Some((parts[0].to_string(), parts[1].to_string()))
                        } else {
                            None
                        }
                    })
                    .unzip();

                let github_token = std::env::var("GITHUB_TOKEN").ok();

                if github_token.is_none() {
                    warn!("--store-github-attestation requires GITHUB_TOKEN environment variable");
                }
                if repo_owner.is_none() {
                    warn!("--store-github-attestation requires GITHUB_REPOSITORY environment variable");
                }

                AttestationConfig {
                    repo_owner,
                    repo_name,
                    github_token,
                }
            } else {
                // Return Sigstore bundle JSON for uploading to prefix.dev
                AttestationConfig {
                    repo_owner: None,
                    repo_name: None,
                    github_token: None,
                }
            };

            match create_attestation(package_file, channel_url.as_str(), &config, &client).await {
                Ok(attestation_bundle_json) => {
                    // Save attestation bundle JSON to a file next to the package
                    tracing::info!("Generated attestation: {}", attestation_bundle_json);
                    let attestation_file = package_file.with_extension("attestation.json");
                    tokio_fs::write(&attestation_file, attestation_bundle_json)
                        .await
                        .into_diagnostic()?;
                    info!("Generated attestation for {}", filename);
                    Some(attestation_file)
                }
                Err(e) => {
                    // If attestation generation was explicitly requested, fail the upload
                    return Err(miette::miette!(
                        "Failed to generate attestation for {}: {}\n\
                         Upload aborted because attestation generation was requested but failed.\n\
                         \n\
                         Troubleshooting:\n\
                         1. Check that you're running in a supported CI environment (GitHub Actions, GitLab CI, etc.)\n\
                         2. For GitHub Actions, ensure you have 'id-token: write' permission\n\
                         3. Verify OIDC token is available and valid",
                        filename, e
                    )
                    .into());
                }
            }
        } else if let AttestationSource::Attestation(path) = &prefix_data.attestation {
            Some(path.clone())
        } else {
            None
        };

        #[cfg(not(feature = "sigstore-sign"))]
        let attestation_path = match &prefix_data.attestation {
            AttestationSource::Attestation(path) => Some(path.clone()),
            _ => None,
        };

        let progress_bar = indicatif::ProgressBar::new(file_size)
            .with_prefix("Uploading")
            .with_style(default_bytes_style().into_diagnostic()?);

        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let mut current_try = 0;
        let request_start = SystemTime::now();

        loop {
            progress_bar.reset();

            let form = create_upload_form(
                package_file,
                &filename,
                file_size,
                progress_bar.clone(),
                &attestation_path,
            )
            .await?;

            let response = get_default_client()
                .into_diagnostic()?
                .post(url.clone())
                .multipart(form)
                .bearer_auth(&token)
                .send()
                .await
                .into_diagnostic()?;

            if response.status().is_success() {
                progress_bar.finish();
                info!("Upload complete for package file: {}", filename);
                break;
            }

            let status = response.status();
            let body = response.text().await.into_diagnostic()?;

            // Non-retry status codes
            match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    return Err(PrefixUploadError::AuthenticationFailed {
                        status: status.as_u16(),
                        body,
                    });
                }
                StatusCode::CONFLICT => {
                    // skip if package already exists
                    if prefix_data.skip_existing.is_enabled() {
                        progress_bar.finish();
                        info!("Skip existing package: {}", filename);
                        return Ok(());
                    } else {
                        return Err(PrefixUploadError::Conflict { body });
                    }
                }
                StatusCode::UNPROCESSABLE_ENTITY => {
                    return Err(PrefixUploadError::UnprocessableEntity { body });
                }
                StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::PAYLOAD_TOO_LARGE => {
                    return Err(PrefixUploadError::ClientError {
                        status: status.as_u16(),
                        body,
                    });
                }
                _ => {}
            }

            match retry_policy.should_retry(request_start, current_try) {
                RetryDecision::DoNotRetry => {
                    return Err(PrefixUploadError::ServerError {
                        status: status.as_u16(),
                        body,
                    });
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

    info!("Packages successfully uploaded to prefix.dev server");
    Ok(())
}

#[cfg(test)]
mod test {
    use axum::{http::StatusCode, Router};
    use rattler_networking::AuthenticationStorage;

    use super::{upload_package_to_prefix, PrefixUploadError};
    use crate::upload::opt::{AttestationSource, ForceOverwrite, PrefixData, SkipExisting};
    use crate::upload::test_utils::{start_test_server, test_package_path};

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

    fn make_prefix_data(url: url::Url, skip_existing: bool) -> PrefixData {
        PrefixData::new(
            url,
            "test-channel".to_string(),
            Some("test-token".to_string()),
            AttestationSource::NoAttestation,
            SkipExisting(skip_existing),
            ForceOverwrite(false),
            false,
        )
    }

    #[tokio::test]
    async fn test_prefix_upload_success() {
        let router = Router::new().fallback(ok_with_bearer);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let prefix_data = make_prefix_data(url, false);
        let result =
            upload_package_to_prefix(&storage, &vec![test_package_path()], prefix_data).await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_prefix_upload_skip_existing() {
        let router = Router::new().fallback(conflict);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let prefix_data = make_prefix_data(url, true);
        let result =
            upload_package_to_prefix(&storage, &vec![test_package_path()], prefix_data).await;
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_prefix_upload_conflict_without_skip() {
        let router = Router::new().fallback(conflict);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let prefix_data = make_prefix_data(url, false);
        let err = upload_package_to_prefix(&storage, &vec![test_package_path()], prefix_data)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrefixUploadError::Conflict { .. }),
            "expected Conflict, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_prefix_upload_auth_failure() {
        let router = Router::new().fallback(unauthorized);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let prefix_data = make_prefix_data(url, false);
        let err = upload_package_to_prefix(&storage, &vec![test_package_path()], prefix_data)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                PrefixUploadError::AuthenticationFailed { status: 401, .. }
            ),
            "expected AuthenticationFailed, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_prefix_upload_missing_api_key() {
        let router = Router::new().fallback(ok_with_bearer);
        let url = start_test_server(router).await;
        let storage = AuthenticationStorage::empty();
        let prefix_data = PrefixData::new(
            url,
            "test-channel".to_string(),
            None,
            AttestationSource::NoAttestation,
            SkipExisting(false),
            ForceOverwrite(false),
            false,
        );
        let err = upload_package_to_prefix(&storage, &vec![test_package_path()], prefix_data)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrefixUploadError::MissingApiKey),
            "expected MissingApiKey, got: {err:?}"
        );
    }
}
