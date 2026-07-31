use std::path::{Path, PathBuf};

use futures::{StreamExt, TryStreamExt};
use miette::IntoDiagnostic;
use opendal::{Configurator, ErrorKind, Operator};
use rattler_azure::AzureCredentials;
use tokio::io::AsyncReadExt;
use tokio_util::bytes::BytesMut;
use url::Url;

use crate::upload::package::ExtractedPackage;

/// Size of a single block handed to the writer. Azure Blob block uploads keep
/// the number of blocks low with larger blocks; we use 10 MiB.
const DESIRED_CHUNK_SIZE: usize = 1024 * 1024 * 10;

/// Number of blocks of a single package that are uploaded concurrently.
const PART_CONCURRENCY: usize = 4;

/// Number of packages that are uploaded concurrently.
const PACKAGE_CONCURRENCY: usize = 4;

/// SAS permissions requested when minting a user-delegation SAS for uploads.
/// Creating and writing blobs needs `c` + `w`; `r` is required on top of those
/// because the overwrite guard `stat`s each blob before writing it, and a
/// `stat` (HEAD Blob) is a read. The SAS stays container-scoped and short-lived.
pub(crate) const AZURE_UPLOAD_SAS_PERMISSIONS: &str = "rcw";

/// Uploads packages to a channel in an Azure Blob Storage container.
///
/// The channel URL must be of the form
/// `https://<account>.blob.core.windows.net/<container>/<prefix>`; the account
/// name, endpoint, container, and root prefix are all derived from it (see
/// `azblob_config`). Because the account is derived from the host, upload
/// requires this dotted `<account>.blob...` form and does not support
/// path-style or emulator (Azurite) endpoints. The full blob host lives in the
/// channel URL itself, so no separate account/endpoint configuration is needed.
/// The [`AzureCredentials`] supply only the account key or SAS token.
pub async fn upload_package_to_azure(
    channel: Url,
    credentials: AzureCredentials,
    package_files: &[PathBuf],
    force: bool,
) -> miette::Result<()> {
    let config = rattler_azure::azblob_config(&credentials, &channel).into_diagnostic()?;

    let builder = config.into_builder();
    let op = Operator::new(builder).into_diagnostic()?.finish();

    // Upload multiple packages concurrently. Each package is written to its own
    // key, so the individual uploads are independent. The first failure aborts
    // the remaining uploads rather than letting them run to completion.
    futures::stream::iter(package_files.iter())
        .map(Ok)
        .try_for_each_concurrent(PACKAGE_CONCURRENCY, |package_file| {
            let op = op.clone();
            let channel = &channel;
            async move { upload_single_package(&op, channel, package_file, force).await }
        })
        .await?;

    Ok(())
}

/// Uploads a single package file to the Azure Blob container via the given operator.
async fn upload_single_package(
    op: &Operator,
    channel: &Url,
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

    // The blob's on-the-wire address, used only for diagnostics. `channel.path()`
    // already carries `/<container>/<prefix>`, so the full `az://` URL is the host
    // followed by that path and the key; do not prepend the container again.
    let blob_url = format!(
        "az://{}{}/{key}",
        channel.host_str().unwrap_or_default(),
        channel.path()
    );

    // Guard against overwriting an existing blob when `--force` was not passed.
    // opendal 0.57 only honours `if_not_exists` on the single-shot Put Blob path,
    // never the multi-block Put Block List path used for packages larger than a
    // single block, so the writer-level `if_not_exists(!force)` below silently
    // does nothing for large uploads. An explicit `stat` closes that gap at all
    // sizes. The residual stat->write TOCTOU (another writer could create the
    // blob between this check and `close`) is acceptable: it is strictly better
    // than a silent clobber, and the writer-level `if_not_exists` still
    // guards the small-file and racing-writer cases.
    if !force {
        match op.stat(&key).await {
            Ok(_) => {
                miette::bail!("Package {blob_url} already exists. Use --force to overwrite.");
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e).into_diagnostic(),
        }
    }

    // The streaming loop below needs the package size to chunk the upload.
    let size = fs_err::tokio::metadata(package_file)
        .await
        .into_diagnostic()?
        .len();
    let mut file = tokio::io::BufReader::new(
        fs_err::tokio::File::open(package_file)
            .await
            .into_diagnostic()?,
    );

    // Construct a writer for the package. Setting `chunk` and `concurrent`
    // enables opendal's concurrent block upload: data is buffered into
    // `DESIRED_CHUNK_SIZE` blocks and up to `PART_CONCURRENCY` blocks are
    // uploaded in parallel. `if_not_exists(!force)` maps to an `If-None-Match: *`
    // precondition so an existing blob is not silently overwritten unless
    // `--force` was passed; note opendal 0.57 only honours it on the single-shot
    // Put Blob path, so the pre-write `stat` above is what guards large uploads.
    let mut writer = match op
        .writer_with(&key)
        .chunk(DESIRED_CHUNK_SIZE)
        .concurrent(PART_CONCURRENCY)
        .if_not_exists(!force)
        .await
    {
        Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
            miette::bail!("Package {blob_url} already exists. Use --force to overwrite.");
        }
        Ok(writer) => writer,
        Err(e) => {
            return Err(e).into_diagnostic();
        }
    };

    // Stream the file to the writer in `DESIRED_CHUNK_SIZE` chunks. opendal takes
    // care of buffering these into correctly sized blocks and uploading them
    // concurrently.
    let mut remaining_size = size as usize;
    while remaining_size > 0 {
        // Allocate memory for this chunk.
        let chunk_size = remaining_size.min(DESIRED_CHUNK_SIZE);
        let mut chunk = BytesMut::with_capacity(chunk_size);
        // Zero-fill up to `chunk_size`; `read_exact` below overwrites every byte.
        chunk.resize(chunk_size, 0);

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
            miette::bail!("Package {blob_url} already exists. Use --force to overwrite.");
        }
        Ok(_) => {
            tracing::info!("Uploaded package to {blob_url}");
        }
        Err(e) => {
            return Err(e).into_diagnostic();
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use opendal::{Operator, services::Memory};
    use url::Url;

    use super::upload_single_package;
    use crate::upload::package::ExtractedPackage;
    use crate::upload::test_utils::test_package_path;

    fn memory_operator() -> Operator {
        Operator::new(Memory::default()).unwrap().finish()
    }

    fn test_channel() -> Url {
        Url::parse("https://account.blob.core.windows.net/container/prefix").unwrap()
    }

    fn package_key() -> String {
        let path = test_package_path();
        let package = ExtractedPackage::from_package_file(&path).unwrap();
        format!(
            "{}/{}",
            package.subdir().unwrap(),
            package.filename().unwrap()
        )
    }

    /// C2: without `--force`, uploading over an existing blob must error rather
    /// than silently overwrite it, at all package sizes. This exercises the
    /// explicit pre-write `stat` guard that backstops opendal's
    /// `if_not_exists`, which is dropped on the multi-block upload path.
    #[tokio::test]
    async fn test_existing_blob_without_force_errors() {
        let op = memory_operator();
        let channel = test_channel();
        let package = test_package_path();

        // Seed the target blob so the next upload finds it already present.
        upload_single_package(&op, &channel, &package, true)
            .await
            .expect("initial force upload should succeed");

        let err = upload_single_package(&op, &channel, &package, false)
            .await
            .expect_err("upload over an existing blob without --force must fail");
        assert!(
            err.to_string().contains("already exists"),
            "unexpected error: {err}"
        );
    }

    /// A non-forced upload into an empty container succeeds.
    #[tokio::test]
    async fn test_upload_into_empty_container_succeeds() {
        let op = memory_operator();
        upload_single_package(&op, &test_channel(), &test_package_path(), false)
            .await
            .expect("upload into an empty container should succeed");

        let meta = op.stat(&package_key()).await.unwrap();
        let expected_size = std::fs::metadata(test_package_path()).unwrap().len();
        assert_eq!(meta.content_length(), expected_size);
    }
}
