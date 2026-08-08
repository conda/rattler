use std::path::{Path, PathBuf};

use futures::StreamExt;
use miette::IntoDiagnostic;
use opendal::{Configurator, services::S3Config};
use rattler_s3::ResolvedS3Credentials;
use url::Url;

use crate::upload::{
    object_store::{
        BlobStore, BlobUploadTarget, PACKAGE_CONCURRENCY, stream_package_to_object_store,
    },
    opt::ForceOverwrite,
    package::ExtractedPackage,
};

/// Uploads a package to a channel in an S3 bucket.
///
/// Credentials must already be resolved by the caller (e.g. via
/// [`rattler_s3::S3Credentials::resolve`] or
/// [`ResolvedS3Credentials::from_sdk`]).
pub async fn upload_package_to_s3(
    channel: Url,
    credentials: ResolvedS3Credentials,
    package_files: &[PathBuf],
    force: bool,
) -> miette::Result<()> {
    let bucket = channel
        .host_str()
        .ok_or(miette::miette!("No bucket in S3 URL"))?;

    // Create the S3 configuration for opendal.
    let mut s3_config = S3Config::default();
    s3_config.root = Some(channel.path().to_string());
    s3_config.bucket = bucket.to_string();

    s3_config.endpoint = Some(credentials.endpoint_url.to_string());
    s3_config.region = Some(credentials.region);
    s3_config.access_key_id = Some(credentials.access_key_id);
    s3_config.secret_access_key = Some(credentials.secret_access_key);
    s3_config.session_token = credentials.session_token;
    s3_config.enable_virtual_host_style =
        credentials.addressing_style == rattler_s3::S3AddressingStyle::VirtualHost;

    let builder = s3_config.into_builder();
    let op = BlobStore::new(builder).into_diagnostic()?;

    // Upload multiple packages concurrently. Each individual package upload also
    // streams its chunks concurrently (see `upload_single_package`).
    futures::stream::iter(package_files.iter())
        .map(|package_file| {
            let op = op.clone();
            let channel = &channel;
            async move {
                upload_single_package(&op, channel, bucket, package_file, force.into()).await
            }
        })
        .buffer_unordered(PACKAGE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<miette::Result<Vec<_>>>()?;

    Ok(())
}

/// Uploads a single package file to the S3 bucket via the given operator.
async fn upload_single_package(
    op: &BlobStore,
    channel: &Url,
    bucket: &str,
    package_file: &Path,
    force: ForceOverwrite,
) -> miette::Result<()> {
    let package = ExtractedPackage::from_package_file(package_file)?;
    let target = BlobUploadTarget::from_package(&package)?;
    let destination = format!("s3://{bucket}{}/{}", channel.path(), target.key());

    stream_package_to_object_store(op, &target, package_file, &destination, force).await
}
