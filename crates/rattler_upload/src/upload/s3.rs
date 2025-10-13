use std::path::PathBuf;

use miette::IntoDiagnostic;
use opendal::{services::S3Config, Configurator, ErrorKind, Operator};
use rattler_digest::{HashingReader, Md5, Sha256};
use rattler_networking::AuthenticationStorage;
use rattler_s3::{ResolvedS3Credentials, S3Credentials};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::bytes::BytesMut;
use url::Url;

use crate::upload::package::ExtractedPackage;

const DESIRED_CHUNK_SIZE: usize = 1024 * 1024 * 10;

/// Uploads a package to a channel in an S3 bucket.
#[allow(clippy::too_many_arguments)]
pub async fn upload_package_to_s3(
    auth_storage: &AuthenticationStorage,
    channel: Url,
    credentials: Option<S3Credentials>,
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

    // Resolve the credentials to use.
    let resolved_credentials = match credentials {
        Some(credentials) => credentials
            .resolve(&channel, auth_storage)
            .ok_or_else(|| miette::miette!("Could not find S3 credentials in the authentication storage, and no credentials were provided via the command line."))?,
        None => {
            ResolvedS3Credentials::from_sdk().await.into_diagnostic()?
        }
    };

    s3_config.endpoint = Some(resolved_credentials.endpoint_url.to_string());
    s3_config.region = Some(resolved_credentials.region);
    s3_config.access_key_id = Some(resolved_credentials.access_key_id);
    s3_config.secret_access_key = Some(resolved_credentials.secret_access_key);
    s3_config.session_token = resolved_credentials.session_token;
    s3_config.enable_virtual_host_style =
        resolved_credentials.addressing_style == rattler_s3::S3AddressingStyle::VirtualHost;

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

        // Construct a writer for the package.
        let mut writer = match op
            .writer_with(&key)
            .content_disposition(&format!("attachment; filename={filename}"))
            .if_not_exists(!force)
            .user_metadata([
                (String::from("package-sha256"), format!("{sha256hash:x}")),
                (String::from("package-md5"), format!("{md5hash:x}")),
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

        // Write the contents to the writer. We do this in a more complex way than just
        // using `io::copy` because some underlying storage providers expect to receive
        // the data in specifically sized chunks. The code below guarantees chunks of
        // equal size except for maybe the last chunk.
        let mut remaining_size = size as usize;
        loop {
            // Allocate memory for this chunk
            let chunk_size = remaining_size.min(DESIRED_CHUNK_SIZE);
            let mut chunk = BytesMut::with_capacity(chunk_size);
            // SAFE: because we do not care about the bytes that are currently in the buffer
            unsafe { chunk.set_len(chunk_size) };

            // Fill the chunk with data. This reads exactly the number of bytes we want. No
            // more, no less.
            let bytes_read = file.read_exact(&mut chunk[..]).await.into_diagnostic()?;
            debug_assert_eq!(bytes_read, chunk.len());

            // Write the writes directly to storage
            writer.write(chunk.freeze()).await.into_diagnostic()?;

            // Update the number of remaining bytes
            remaining_size = remaining_size.saturating_sub(bytes_read);
            if remaining_size == 0 {
                break;
            }
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
    }

    Ok(())
}
