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
) -> miette::Result<()> {
    let check_storage = || {
        match storage.get_by_url(Url::from(prefix_data.url.clone())) {
            Ok((_, Some(Authentication::BearerToken(token)))) => Ok(token),
            Ok((_, Some(_))) => {
                Err(miette::miette!("A Conda token is required for authentication with prefix.dev.
                        Authentication information found in the keychain / auth file, but it was not a Bearer token"))
            }
            Ok((_, None)) => {
                Err(miette::miette!(
                    "No prefix.dev api key was given and none was found in the keychain / auth file"
                ))
            }
            Err(e) => {
                Err(miette::miette!(
                    "Failed to get authentication information from keychain: {e}"
                ))
            }
        }
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
        return Err(miette::miette!(
            "Attestation generation was requested, but the 'sigstore' feature is not enabled.\n\
             Please rebuild with the 'sigstore' feature enabled."
        ));
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
                    return Err(miette::miette!(
                        "Attestation was requested, but trusted publishing is not configured"
                    ));
                }
                (check_storage()?, false)
            }
            TrustedPublishResult::Ignored(err) => {
                tracing::warn!("Checked for trusted publishing but failed with {err}");
                if wants_attestation {
                    return Err(miette::miette!(
                        "Attestation was requested, but trusted publishing is not configured"
                    ));
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
                    return Err(miette::miette!(
                        "Attestation was requested, but trusted publishing is not configured"
                    ));
                }
                check_storage()?
            }
            TrustedPublishResult::Ignored(err) => {
                tracing::warn!("Checked for trusted publishing but failed with {err}");
                if wants_attestation {
                    return Err(miette::miette!(
                        "Attestation was requested, but trusted publishing is not configured"
                    ));
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
                    ));
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
            let err = miette::miette!(
                "Failed to upload package file: {}\nStatus: {}\nBody: {}",
                package_file.display(),
                status,
                body
            );

            // Non-retry status codes (identical to send_request_with_retry)
            match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    return Err(miette::miette!("Authentication error: {}", err));
                }
                StatusCode::CONFLICT => {
                    // skip if package already exists
                    if prefix_data.skip_existing.is_enabled() {
                        progress_bar.finish();
                        info!("Skip existing package: {}", filename);
                        return Ok(());
                    } else {
                        return Err(miette::miette!("Resource conflict: {}", err));
                    }
                }
                StatusCode::UNPROCESSABLE_ENTITY => {
                    return Err(miette::miette!("Resource conflict: {}", err));
                }
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

    info!("Packages successfully uploaded to prefix.dev server");
    Ok(())
}
