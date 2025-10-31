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

use super::opt::{
    // ← Import from sibling module
    PrefixData,
};

use crate::upload::{
    attestation::{create_attestation_with_cosign, AttestationConfig},
    default_bytes_style, get_client_with_retry, get_default_client,
    trusted_publishing::{check_trusted_publishing, get_raw_oidc_token, TrustedPublishResult},
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

    // Check if we're using trusted publishing and if we should generate attestations
    let (token, should_generate_attestation, oidc_token) = match prefix_data.api_key {
        Some(api_key) => (api_key, false, None),
        None => match check_trusted_publishing(&client, &prefix_data.url).await {
            TrustedPublishResult::Configured(token) => {
                // Get the OIDC token for attestation generation
                let oidc_token = if prefix_data.generate_attestation {
                    match get_raw_oidc_token(&client).await {
                        Ok(token) => Some(token),
                        Err(e) => {
                            tracing::warn!("Failed to get OIDC token for attestation: {e}");
                            None
                        }
                    }
                } else {
                    None
                };
                (token.secret().to_string(), oidc_token.is_some(), oidc_token)
            }
            TrustedPublishResult::Skipped => {
                if prefix_data.attestation.is_some() || prefix_data.generate_attestation {
                    return Err(miette::miette!(
                        "Attestation was requested, but trusted publishing is not configured"
                    ));
                }
                (check_storage()?, false, None)
            }
            TrustedPublishResult::Ignored(err) => {
                tracing::warn!("Checked for trusted publishing but failed with {err}");
                if prefix_data.attestation.is_some() || prefix_data.generate_attestation {
                    return Err(miette::miette!(
                        "Attestation was requested, but trusted publishing is not configured"
                    ));
                }
                (check_storage()?, false, None)
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
        let url = prefix_data
            .url
            .join(&format!("api/v1/upload/{}", prefix_data.channel))
            .into_diagnostic()?;

        // Generate attestation if we're using trusted publishing and it was requested
        let attestation_path = if should_generate_attestation && oidc_token.is_some() {
            let channel_url = prefix_data
                .url
                .join(&prefix_data.channel)
                .into_diagnostic()?;

            // Build attestation configuration. We deliberately avoid providing a GitHub token
            // so we always return a Sigstore bundle JSON for uploading to prefix.dev.
            let config = AttestationConfig {
                repo_owner: None,
                repo_name: None,
                github_token: None,
                use_github_oidc: true,
                cosign_private_key: std::env::var("COSIGN_KEY_PATH").ok(),
            };

            match create_attestation_with_cosign(
                package_file,
                channel_url.as_str(),
                &config,
                &client,
            )
            .await
            {
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
                         1. Ensure cosign is installed: pixi global install cosign\n\
                         2. Check that you're running in GitHub Actions with 'id-token: write' permission\n\
                         3. Verify OIDC token is available and valid\n\
                         4. Check cosign logs above for specific errors",
                        filename, e
                    ));
                }
            }
        } else {
            prefix_data.attestation.clone()
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
                    // skip if package is existed
                    if prefix_data.skip_existing {
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
