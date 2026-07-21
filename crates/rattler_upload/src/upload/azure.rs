use std::path::{Path, PathBuf};

use futures::StreamExt;
use miette::IntoDiagnostic;
use opendal::{Configurator, ErrorKind, Operator, services::AzblobConfig};
use rattler_azure::AzureCredentials;
use rattler_digest::{HashingReader, Md5, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::bytes::BytesMut;
use url::Url;

use crate::upload::package::ExtractedPackage;

/// Size of a single chunk handed to the writer. Azure Blob storage expects data
/// to be uploaded in reasonably sized blocks; we buffer into 10 MiB chunks.
const DESIRED_CHUNK_SIZE: usize = 1024 * 1024 * 10;

/// Number of packages that are uploaded concurrently.
const PACKAGE_CONCURRENCY: usize = 4;

/// Uploads packages to a channel in an Azure Blob Storage container.
///
/// The channel URL is expected to be of the form
/// `https://<account>.blob.core.windows.net/<container>/<prefix>`; the account
/// name, endpoint, container, and root prefix are all derived from it (see
/// [`azblob_config`]). The [`AzureCredentials`] supply only the account key or
/// SAS token.
pub async fn upload_package_to_azure(
    channel: Url,
    credentials: AzureCredentials,
    package_files: &[PathBuf],
    force: bool,
) -> miette::Result<()> {
    let config = azblob_config(&credentials, &channel)?;
    let container = config.container.clone();

    let builder = config.into_builder();
    let op = Operator::new(builder).into_diagnostic()?.finish();

    // Upload multiple packages concurrently. Each package is written to its own
    // key, so the individual uploads are independent.
    futures::stream::iter(package_files.iter())
        .map(|package_file| {
            let op = op.clone();
            let channel = &channel;
            let container = container.as_str();
            async move { upload_single_package(&op, channel, container, package_file, force).await }
        })
        .buffer_unordered(PACKAGE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<miette::Result<Vec<_>>>()?;

    Ok(())
}

/// Uploads a single package file to the Azure Blob container via the given operator.
async fn upload_single_package(
    op: &Operator,
    channel: &Url,
    container: &str,
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

    // Construct a writer for the package. `if_not_exists(!force)` maps to an
    // `If-None-Match: *` precondition so an existing blob is not silently
    // overwritten unless `--force` was passed.
    let mut writer = match op
        .writer_with(&key)
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
                "Package az://{container}{}/{key} already exists. Use --force to overwrite.",
                channel.path().to_string()
            );
        }
        Ok(writer) => writer,
        Err(e) => {
            return Err(e).into_diagnostic();
        }
    };

    // Stream the file to the writer in `DESIRED_CHUNK_SIZE` chunks. We do this in
    // a more complex way than a plain `io::copy` because the underlying storage
    // provider expects to receive the data in specifically sized chunks. The code
    // below guarantees chunks of equal size except for maybe the last chunk.
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

        // Hand the chunk to the writer.
        writer.write(chunk.freeze()).await.into_diagnostic()?;

        // Update the number of remaining bytes.
        remaining_size = remaining_size.saturating_sub(bytes_read);
    }

    match writer.close().await {
        Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
            miette::bail!(
                "Package az://{container}{}/{key} already exists. Use --force to overwrite.",
                channel.path().to_string()
            );
        }
        Ok(_) => {
            tracing::info!(
                "Uploaded package to az://{container}{}/{key}",
                channel.path().to_string()
            );
        }
        Err(e) => {
            return Err(e).into_diagnostic();
        }
    }

    Ok(())
}

/// Build an opendal [`AzblobConfig`] from a channel URL and credentials.
///
/// The account name, endpoint, container, and root prefix are all derived from
/// the URL (`https://<account>.blob.core.windows.net/<container>/<prefix>`); the
/// credentials supply only the account key or SAS token.
fn azblob_config(credentials: &AzureCredentials, channel: &Url) -> miette::Result<AzblobConfig> {
    let host = channel
        .host_str()
        .ok_or_else(|| miette::miette!("No host in Azure blob URL"))?;
    let account_name = host
        .split('.')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| miette::miette!("Could not derive account name from Azure blob URL"))?;

    let mut segments = channel
        .path_segments()
        .ok_or_else(|| miette::miette!("No path in Azure blob URL"))?;
    let container = segments
        .next()
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| miette::miette!("No container in Azure blob URL"))?;
    let root = format!("/{}", segments.collect::<Vec<_>>().join("/"));

    // Preserve a non-default port so custom endpoints (e.g. the Azurite
    // emulator on :10000) work; real Azure uses the scheme default (443).
    let authority = match channel.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };

    let (account_key, sas_token) = match credentials {
        AzureCredentials::AccountKey(key) => (Some(key.clone()), None),
        AzureCredentials::SasToken(token) => (None, Some(token.clone())),
    };

    Ok(AzblobConfig {
        endpoint: Some(format!("{}://{}", channel.scheme(), authority)),
        account_name: Some(account_name.to_string()),
        container: container.to_string(),
        root: Some(root),
        account_key,
        sas_token,
        ..Default::default()
    })
}
