use std::path::PathBuf;

use aws_config::{meta::region::RegionProviderChain, BehaviorVersion};
use aws_sdk_s3::config::{Credentials, ProvideCredentials};
use miette::{Context, IntoDiagnostic};
use opendal::{
    services::{S3Config, S3},
    Configurator, ErrorKind, Operator,
};
use rattler_networking::{Authentication, AuthenticationStorage};
use url::Url;

use crate::upload::package::ExtractedPackage;

/// Uploads a package to a channel in an S3 bucket.
#[allow(clippy::too_many_arguments)]
pub async fn upload_package_to_s3(
    auth_storage: &AuthenticationStorage,
    channel: Url,
    endpoint_url: Option<Url>,
    region: Option<String>,
    force_path_style: Option<bool>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
    package_files: &Vec<PathBuf>,
    force: bool,
) -> miette::Result<()> {
    let bucket = channel
        .host_str()
        .ok_or(miette::miette!("No bucket in S3 URL"))?;

    // Create the S3 configuration for opendal.
    let mut s3_config = S3Config::default();
    s3_config.root = Some(channel.path().to_string());
    s3_config.bucket = bucket.to_string();

    // Determine region and endpoint URL.
    let endpoint = endpoint_url
        .map(|url| url.to_string())
        .or_else(|| std::env::var("AWS_ENDPOINT_URL").ok())
        .unwrap_or_else(|| String::from("https://s3.amazonaws.com"));

    let mut region = region;
    if region.is_none() {
        // Try to use the AWS SDK to determine the region.
        let region_provider = RegionProviderChain::default_provider();
        region = region_provider.region().await.map(|r| r.to_string());
    }
    if region.is_none() {
        // If no region is provided, we try to detect it from the endpoint URL.
        region = S3::detect_region(&endpoint, &s3_config.bucket).await;
    }
    s3_config.region = region;
    s3_config.endpoint = Some(endpoint);

    // How to access the S3 bucket.
    s3_config.enable_virtual_host_style = force_path_style.is_none_or(|x| !x);

    // Use credentials from the CLI if they are provided.
    if let (Some(access_key_id), Some(secret_access_key)) = (access_key_id, secret_access_key) {
        s3_config.secret_access_key = Some(secret_access_key);
        s3_config.access_key_id = Some(access_key_id);
        s3_config.session_token = session_token;
    } else if let Some((access_key_id, secret_access_key, session_token)) =
        load_s3_credentials_from_auth_storage(auth_storage, channel.clone())?
    {
        // Use the credentials from the authentication storage if they are available.
        s3_config.access_key_id = Some(access_key_id);
        s3_config.secret_access_key = Some(secret_access_key);
        s3_config.session_token = session_token;
    } else {
        let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
        let Some(credentials_provider) = config.credentials_provider() else {
            return Err(miette::miette!("No AWS credentials provider found",));
        };
        let credentials: Credentials = credentials_provider
            .provide_credentials()
            .await
            .into_diagnostic()
            .context("failed to determine AWS credentials")?;
        s3_config.access_key_id = Some(credentials.access_key_id().to_string());
        s3_config.secret_access_key = Some(credentials.secret_access_key().to_string());
        s3_config.session_token = credentials.session_token().map(ToString::to_string);
    }

    let builder = s3_config.into_builder();
    let op = Operator::new(builder).into_diagnostic()?.finish();

    for package_file in package_files {
        let package = ExtractedPackage::from_package_file(package_file)?;
        let subdir = package
            .subdir()
            .ok_or_else(|| miette::miette!("Failed to get subdir"))?;
        let filename = package
            .filename()
            .ok_or_else(|| miette::miette!("Failed to get filename"))?;
        let key = format!("{subdir}/{filename}");
        let body = fs_err::tokio::read(package_file).await.into_diagnostic()?;
        match op
            .write_with(&key, body)
            .content_disposition(&format!("attachment; filename={filename}"))
            .if_not_exists(!force)
            .await
        {
            Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
                tracing::info!(
                    "Skipped package s3://{bucket}{}/{key}, the package already exists. Use --force to overwrite.",
                    channel.path().to_string()
                );
            }
            Ok(_metadata) => {
                tracing::info!(
                    "Uploaded package to s3://{bucket}{}/{key}",
                    channel.path().to_string()
                );
            }
            Err(e) => return Err(e).into_diagnostic(),
        }
    }

    Ok(())
}

fn load_s3_credentials_from_auth_storage(
    auth_storage: &AuthenticationStorage,
    channel: Url,
) -> miette::Result<Option<(String, String, Option<String>)>> {
    let auth = auth_storage.get_by_url(channel).into_diagnostic()?;
    if let (
        _,
        Some(Authentication::S3Credentials {
            access_key_id,
            secret_access_key,
            session_token,
        }),
    ) = auth
    {
        Ok(Some((access_key_id, secret_access_key, session_token)))
    } else {
        Ok(None)
    }
}
