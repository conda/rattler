use std::path::PathBuf;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use miette::IntoDiagnostic;
use opendal::{services::S3Config, Configurator, ErrorKind, Operator};
use rattler_networking::AuthenticationStorage;
use rattler_s3::{ResolvedS3Credentials, S3Credentials};
use url::Url;

use crate::upload::package::ExtractedPackage;

/// Uploads a package to a channel in an S3 bucket with progress tracking.
#[allow(clippy::too_many_arguments)]
pub async fn upload_package_to_s3(
    auth_storage: &AuthenticationStorage,
    channel: Url,
    credentials: Option<S3Credentials>,
    package_files: &Vec<PathBuf>,
    region: Option<String>,
    force: bool,
) -> miette::Result<()> {
    let bucket = channel
        .host_str()
        .ok_or(miette::miette!("No bucket in S3 URL"))?;

    // Create progress tracking
    let multi_progress = MultiProgress::new();
    let main_progress = multi_progress.add(ProgressBar::new(package_files.len() as u64));
    main_progress.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    main_progress.set_message("Preparing upload...");

    // Create the S3 configuration for opendal.
    let mut s3_config = S3Config::default();
    s3_config.root = Some(channel.path().to_string());
    s3_config.bucket = bucket.to_string();

    // Resolve the credentials to use.
    let resolved_credentials = match credentials {
        Some(credentials) => credentials
            .resolve(&channel, auth_storage)
            .ok_or_else(|| miette::miette!("Could not find S3 credentials in the authentication storage, and no credentials were provided via the command line."))?,
        None => {
            ResolvedS3Credentials::from_sdk().await.into_diagnostic()?
        }
    };

    if let Some(r) = region {
        s3_config.region = Some(r);
    } else {
        s3_config.region = Some(resolved_credentials.region);
    }

    s3_config.endpoint = Some(resolved_credentials.endpoint_url.to_string());
    s3_config.access_key_id = Some(resolved_credentials.access_key_id);
    s3_config.secret_access_key = Some(resolved_credentials.secret_access_key);
    s3_config.session_token = resolved_credentials.session_token;
    s3_config.enable_virtual_host_style =
        resolved_credentials.addressing_style == rattler_s3::S3AddressingStyle::VirtualHost;

    let builder = s3_config.into_builder();
    let op = Operator::new(builder).into_diagnostic()?.finish();

    main_progress.set_message("Uploading packages...");

    for (index, package_file) in package_files.iter().enumerate() {
        let package = ExtractedPackage::from_package_file(package_file)?;
        let subdir = package
            .subdir()
            .ok_or_else(|| miette::miette!("Failed to get subdir"))?;
        let filename = package
            .filename()
            .ok_or_else(|| miette::miette!("Failed to get filename"))?;
        let key = format!("{subdir}/{filename}");

        // Create individual progress bar for current file
        let file_progress = multi_progress.add(ProgressBar::new_spinner());
        file_progress.set_style(
            ProgressStyle::default_spinner()
                .template("  {spinner:.cyan} {msg}")
                .unwrap(),
        );
        file_progress.set_message(format!("Uploading {filename}..."));

        let body = fs_err::tokio::read(package_file).await.into_diagnostic()?;

        match op
            .write_with(&key, body)
            .content_disposition(&format!("attachment; filename={filename}"))
            .if_not_exists(!force)
            .await
        {
            Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
                file_progress
                    .finish_with_message(format!("⚠️  Skipped {filename} (already exists)"));
                tracing::info!(
                    "Skipped package s3://{bucket}{}/{key}, the package already exists. Use --force to overwrite.",
                    channel.path().to_string()
                );
            }
            Ok(_metadata) => {
                file_progress.finish_with_message(format!("✅ Uploaded {filename}"));
                tracing::info!(
                    "Uploaded package to s3://{bucket}{}/{key}",
                    channel.path().to_string()
                );
            }
            Err(e) => {
                file_progress.finish_with_message(format!("❌ Failed {filename}"));
                return Err(e).into_diagnostic();
            }
        }

        main_progress.inc(1);
        main_progress.set_message(format!("Progress: {}/{}", index + 1, package_files.len()));
    }

    main_progress.finish_with_message("✅ All packages uploaded successfully!");

    Ok(())
}
