use std::path::{Path, PathBuf};

use futures::StreamExt;
use miette::IntoDiagnostic;
use opendal::{Configurator, ErrorKind, Operator, services::S3Config};
use rattler_digest::{HashingReader, Md5, Sha256};
use rattler_s3::ResolvedS3Credentials;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::bytes::BytesMut;
use url::Url;

use crate::upload::package::ExtractedPackage;

/// Size of a single multipart chunk. S3 requires every part except the last to
/// be at least 5 MiB; we use a larger value to keep the number of parts low.
const DESIRED_CHUNK_SIZE: usize = 1024 * 1024 * 10;

/// Number of chunks of a single package that are uploaded concurrently.
const PART_CONCURRENCY: usize = 4;

/// Number of packages that are uploaded concurrently.
const PACKAGE_CONCURRENCY: usize = 4;

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
    let op = Operator::new(builder).into_diagnostic()?.finish();

    // Upload multiple packages concurrently. Each individual package upload also
    // streams its chunks concurrently (see `upload_single_package`).
    futures::stream::iter(package_files.iter())
        .map(|package_file| {
            let op = op.clone();
            let channel = &channel;
            async move { upload_single_package(&op, channel, bucket, package_file, force).await }
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
    op: &Operator,
    channel: &Url,
    bucket: &str,
    package_file: &Path,
    force: bool,
) -> miette::Result<()> {
    let package = ExtractedPackage::from_package_file(package_file)?;
    let subdir = package
        .subdir()
        .ok_or_else(|| miette::miette!("Failed to get subdir"))?;
    let filename = package
        .filename()
        .ok_or_else(|| miette::miette!("Failed to get filename"))?;
    let key = format!("{subdir}/{filename}");

    // Compute the hash of the package by streaming its content.
    let file = tokio::io::BufReader::new(
        fs_err::tokio::File::open(package_file)
            .await
            .into_diagnostic()?,
    );
    let sha256_reader = HashingReader::<_, Sha256>::new(file);
    let mut md5_reader = HashingReader::<_, Md5>::new(sha256_reader);
    let size = tokio::io::copy(&mut md5_reader, &mut tokio::io::sink())
        .await
        .into_diagnostic()?;
    let (sha256_reader, md5hash) = md5_reader.finalize();
    let (mut file, sha256hash) = sha256_reader.finalize();

    // Rewind the file to the beginning.
    file.rewind().await.into_diagnostic()?;

    // Construct a writer for the package. Setting `chunk` and `concurrent`
    // enables opendal's concurrent multipart upload: data is buffered into
    // `DESIRED_CHUNK_SIZE` parts and up to `PART_CONCURRENCY` parts are uploaded
    // in parallel.
    let mut writer = match op
        .writer_with(&key)
        .chunk(DESIRED_CHUNK_SIZE)
        .concurrent(PART_CONCURRENCY)
        .content_disposition(&format!("attachment; filename={filename}"))
        .if_not_exists(!force)
        .user_metadata([
            (String::from("package-sha256"), hex::encode(sha256hash)),
            (String::from("package-md5"), hex::encode(md5hash)),
        ])
        .await
    {
        Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
            miette::bail!(
                "Package s3://{bucket}{}/{key} already exists. Use --force to overwrite.",
                channel.path().to_string()
            );
        }
        Ok(writer) => writer,
        Err(e) => {
            return Err(e).into_diagnostic();
        }
    };

    // Stream the file to the writer in `DESIRED_CHUNK_SIZE` chunks. opendal takes
    // care of buffering these into correctly sized parts and uploading them
    // concurrently.
    let mut remaining_size = size as usize;
    while remaining_size > 0 {
        // Allocate memory for this chunk.
        let chunk_size = remaining_size.min(DESIRED_CHUNK_SIZE);
        let mut chunk = BytesMut::with_capacity(chunk_size);
        // SAFE: because we do not care about the bytes that are currently in the buffer
        unsafe { chunk.set_len(chunk_size) };

        // Fill the chunk with data. This reads exactly the number of bytes we want. No
        // more, no less.
        let bytes_read = file.read_exact(&mut chunk[..]).await.into_diagnostic()?;
        debug_assert_eq!(bytes_read, chunk.len());

        // Hand the chunk to the writer. With concurrent writes enabled this returns
        // as soon as the chunk is queued rather than fully uploaded.
        writer.write(chunk.freeze()).await.into_diagnostic()?;

        // Update the number of remaining bytes.
        remaining_size = remaining_size.saturating_sub(bytes_read);
    }

    match writer.close().await {
        Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
            miette::bail!(
                "Package s3://{bucket}{}/{key} already exists. Use --force to overwrite.",
                channel.path().to_string()
            );
        }
        Ok(_) => {
            tracing::info!(
                "Uploaded package to s3://{bucket}{}/{key}",
                channel.path().to_string()
            );
        }
        Err(e) => {
            return Err(e).into_diagnostic();
        }
    }

    Ok(())
}
